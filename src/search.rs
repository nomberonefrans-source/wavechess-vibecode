//! Волновые траектории: прямое распространение локальной энергии без обратного
//! сворачивания дерева. Несколько первых ответов образуют суперпозицию; следующие
//! фазы следуют градиенту поля. Неопределённость ослабляет дальний прогноз.
//! Мат достоверен только в текущей позиции или после одного легального хода.
//! Мат в предсказанной траектории остаётся прогнозом, а не доказательством.

use crate::board::*;
pub use crate::cache::WaveCache;
use crate::geometry::*;
use crate::params::Ps;
use crate::settings::EngineConfig;
use crate::wave;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Instant;

pub const MATE: i32 = 30000;
pub const MATE_BOUND: i32 = MATE - 300;

/// Небольшой перевес хода, продолжающего прежний план (устойчивость плана).
const PLAN_BONUS: i32 = 5;
pub const MAX_DEPTH: i32 = 256;
/// Дальние фазы учитываются с накопленной достоверностью траектории.
pub const DEFAULT_DEPTH: i32 = 48;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Report {
    Quiet,
    Uci,
}

#[derive(Clone)]
pub struct Line {
    pub score: i32,
    pub depth: i32,
    pub pv: Vec<Move>,
    pub local_energy: i32,
    pub confidence: f32,
    pub replies: usize,
}

pub struct SearchLimits {
    /// Максимальная длина волновой цепочки (в полуходах).
    pub max_depth: i32,
    pub time_ms: u64, // 0 — без ограничения по времени
    pub variants: usize,
}

pub struct SearchResult {
    pub best: Move,
    pub score: i32,
    pub depth: i32,
    /// Сколько позиций «осело» (вычислено или взято из кэша).
    pub nodes: u64,
    pub elapsed_ms: u64,
    pub lines: Vec<Line>,
    /// Полные передние траектории, включая альтернативные ответы соперника.
    pub continuations: Vec<Vec<Move>>,
}

/// Оценка позиции, в которой сходившему тут же ставят мат.
pub const MATED_NEXT: i32 = -(MATE - 2);

/// Как «осела» позиция `b` с точки зрения стороны, только что сделавшей ход (сотые пешки).
/// Зависит только от позиции и веса угрозы, поэтому её безопасно хранить в кэше по хэшу.
pub fn settle_value(b: &Board, threat_pct: i32) -> i32 {
    let ps = Ps::load();
    let (w, e) = wave::analyze_with(b, &ps);
    let mover = -b.side;
    let mi = if mover == 1 { 0 } else { 1 };
    let oi = 1 - mi;
    let base = e.total * mover as f32;
    let mut loss = 0.0f32;
    let mut legal = MoveList::new();
    b.gen_pseudo(&mut legal, false);
    // Только осуществимые захваты: при шахе возможны и взятие шахующей фигуры,
    // и перекрытие луча с захватом другой фигуры. Псевдоударов недостаточно.
    for &mv in legal.m[..legal.n].iter() {
        let nb = b.make(mv);
        if nb.attacked(nb.ksq[b.side_idx()] as usize, -b.side) {
            continue;
        }
        if nb.in_check() && !has_legal(&nb) {
            return MATED_NEXT;
        }
        if b.sq[mv.to as usize] != 0 || mv.flag == FLAG_EP || mv.promo != 0 {
            loss = loss.max(wave::exchange_capture(b, mv));
        }
    }
    let (mut g1, mut g2) = (0.0f32, 0.0f32);
    for s in 0..64 {
        let p = b.sq[s];
        if p == 0 || p.signum() == mover || p.abs() == KING {
            continue;
        }
        let g = wave::exchange(&w, mi, oi, s, wave::MASS[(p.abs() - 1) as usize]);
        if g > g1 {
            g2 = g1;
            g1 = g;
        } else if g > g2 {
            g2 = g;
        }
    }
    // Две атакованные фигуры не обязательно образуют вилку: один защитный ход
    // может снять обе угрозы. Давление — малая прибавка, не обещанный материал.
    let gain = (ps.counter1 * g1 + ps.counter2 * g2) / (1.0 + loss / 100.0);
    let tw = threat_pct.clamp(0, 300) as f32 / 100.0;
    (base - tw * loss + tw * gain)
        .round()
        .clamp(-20000.0, 20000.0) as i32
}

/// Есть ли в позиции хотя бы один допустимый ход (с ранним выходом).
fn has_legal(b: &Board) -> bool {
    let mut list = MoveList::new();
    b.gen_pseudo(&mut list, false);
    let ui = b.side_idx();
    for i in 0..list.n {
        let nb = b.make(list.m[i]);
        if !nb.attacked(nb.ksq[ui] as usize, -b.side) {
            return true;
        }
    }
    false
}

/// Только один реальный ход: используется для полного показа матовой фазы.
fn mating_step(b: &Board) -> Option<Move> {
    let legal = b.legal_moves();
    legal.m[..legal.n].iter().copied().find(|&mv| {
        let next = b.make(mv);
        next.in_check() && !has_legal(&next)
    })
}

/// Параллельное «отобразить»: результат i-го элемента лежит на i-м месте,
/// поэтому ответ не зависит от числа потоков.
fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, threads: usize, f: F) -> Vec<T> {
    if threads <= 1 || n <= 1 {
        return (0..n).map(|i| f(i)).collect();
    }
    let workers = if threads < n { threads } else { n };
    let next = AtomicUsize::new(0);
    let mut parts: Vec<Vec<(usize, T)>> = Vec::new();
    thread::scope(|sc| {
        let mut handles = Vec::new();
        for _ in 0..workers {
            handles.push(sc.spawn(|| {
                let mut local: Vec<(usize, T)> = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    local.push((i, f(i)));
                }
                local
            }));
        }
        for h in handles {
            parts.push(h.join().unwrap());
        }
    });
    let mut all: Vec<(usize, T)> = parts.into_iter().flatten().collect();
    all.sort_by_key(|p| p.0);
    all.into_iter().map(|p| p.1).collect()
}

/// Ход-кандидат и его оценка с точки зрения ходящего.
struct Cand {
    mv: Move,
    val: i32,
    term: bool, // исход решён правилами: мат, пат, ничья
}

#[derive(Clone, Copy)]
enum End {
    Draw,
    Mate { sign: i32, ply: usize },
}

/// Вектор состояния одной прямой траектории. Никаких оценок границ дерева.
#[derive(Clone)]
struct Chain {
    mv: Move,
    s0: i32,
    moves: Vec<Move>,
    board: Board,
    hashes: Vec<u64>,
    vals: Vec<i32>,
    phase_confidence: Vec<f32>,
    done: Option<End>,
    grp: usize,
    w: f32,
    confidence: f32,
    active: bool,
}

// ---------------------------------------------------------------- решатель

pub struct Searcher {
    cache: WaveCache,
    cfg: EngineConfig,
    counter: AtomicU64,
    start: Instant,
    time_limit_ms: u64,
    stop: Arc<AtomicBool>,
    pub node_limit: u64, // 0 — без ограничения по числу осевших позиций
    report: Report,
    pub report_variants: usize,
    parameter_revision: u64,
}

impl Searcher {
    pub fn new(cfg: &EngineConfig, stop: Arc<AtomicBool>) -> Searcher {
        Searcher {
            cache: WaveCache::new(cfg.cache_mb),
            cfg: *cfg,
            counter: AtomicU64::new(0),
            start: Instant::now(),
            time_limit_ms: 0,
            stop,
            node_limit: 0,
            report: Report::Quiet,
            report_variants: usize::MAX,
            parameter_revision: crate::params::revision(),
        }
    }

    pub fn config(&self) -> EngineConfig {
        self.cfg
    }

    /// Применить новые настройки. Кэш пересоздаётся при смене размера
    /// и очищается при смене веса угрозы (старые значения стали бы неверными).
    pub fn apply_config(&mut self, cfg: &EngineConfig) {
        let resize = cfg.cache_mb != self.cfg.cache_mb;
        let stale = cfg.threat_pct != self.cfg.threat_pct;
        self.cfg = *cfg;
        if resize {
            self.cache = WaveCache::new(cfg.cache_mb);
        } else if stale {
            self.cache.clear();
        }
    }

    /// Забыть всё (новая партия или кнопка Clear Hash).
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    pub fn cache_report(&self) -> String {
        if !self.cache.is_enabled() {
            return "  кэш волновых оценок: выключен\n".to_string();
        }
        let (probes, hits) = self.cache.stats();
        let pct = if probes > 0 {
            hits as f64 * 100.0 / probes as f64
        } else {
            0.0
        };
        format!(
            "  кэш волновых оценок: {} МБ, {} записей, обращений {}, попаданий {:.1}%\n",
            self.cache.bytes() / (1024 * 1024),
            self.cache.entries(),
            probes,
            pct
        )
    }

    fn stop_now(&self) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return true;
        }
        if self.node_limit > 0 && self.counter.load(Ordering::Relaxed) >= self.node_limit {
            return true;
        }
        if self.time_limit_ms > 0 {
            let el = self.start.elapsed().as_millis() as u64;
            if el >= self.time_limit_ms {
                return true;
            }
        }
        false
    }

    /// Осевшая волна позиции (через кэш).
    fn settle(&self, b: &Board) -> i32 {
        self.counter.fetch_add(1, Ordering::Relaxed);
        if let Some(v) = self.cache.get(b.hash) {
            return v;
        }
        let v = settle_value(b, self.cfg.threat_pct);
        self.cache.put(b.hash, v);
        v
    }

    /// Исход позиции `nb` по правилам, если он уже решён (оценка с точки зрения сходившего).
    fn rule_outcome(&self, nb: &Board, game: &[u64], chain: &[u64]) -> Option<i32> {
        if !has_legal(nb) {
            return Some(if nb.in_check() { MATE } else { 0 });
        }
        if nb.halfmove >= 100 || nb.insufficient() {
            return Some(0);
        }
        let mut cnt = 0;
        for &h in game.iter() {
            if h == nb.hash {
                cnt += 1;
            }
        }
        for &h in chain.iter() {
            if h == nb.hash {
                cnt += 1;
            }
        }
        if cnt >= 2 {
            return Some(0); // третье повторение
        }
        None
    }

    /// Внутри траектории незавершённый набор не используется. В корне можно
    /// вернуть лучший из полностью проверенных ходов при совсем малом лимите.
    fn score_moves(
        &self,
        b: &Board,
        game: &[u64],
        chain: &[u64],
        threads: usize,
        interruptible: bool,
    ) -> Vec<Option<Cand>> {
        let legal = b.legal_moves();
        par_map(legal.n, threads, |i| {
            if interruptible && self.stop_now() {
                return None;
            }
            let mv = legal.m[i];
            let nb = b.make(mv);
            Some(match self.rule_outcome(&nb, game, chain) {
                Some(v) => Cand {
                    mv,
                    val: v,
                    term: true,
                },
                None => {
                    let mut val = self.settle(&nb);
                    // Повторный визит уменьшает положительный поток энергии:
                    // движение по замкнутому контуру не приближает выигрыш.
                    // Это не объявление ничьей до третьего повторения.
                    let visits = game.iter().chain(chain).filter(|&&h| h == nb.hash).count();
                    if val > 0 && val < MATE_BOUND && visits > 0 {
                        val = (val as f32 / (1.0 + 0.65 * visits as f32)).round() as i32;
                    }
                    Cand {
                        mv,
                        val,
                        term: false,
                    }
                }
            })
        })
    }

    fn apply_pick(&self, c: &Chain, cand: &Cand, certainty: f32) -> Chain {
        let mut n = c.clone();
        let ply = c.moves.len() + 1;
        let sign = if ply % 2 == 1 { 1 } else { -1 };
        n.board = c.board.make(cand.mv);
        n.moves.push(cand.mv);
        n.hashes.push(n.board.hash);
        // Доверие затухает при неоднозначных продолжениях. Это вес
        // наблюдаемой фазы, а не условие преждевременного обрыва расчёта.
        n.confidence *= 0.94 + 0.06 * certainty;
        if cand.term {
            n.done = Some(if cand.val >= MATE_BOUND {
                End::Mate { sign, ply }
            } else {
                End::Draw
            });
        } else if cand.val <= -MATE_BOUND {
            n.done = Some(End::Mate {
                sign: -sign,
                ply: ply + 1,
            });
            if let Some(mate) = mating_step(&n.board) {
                n.board = n.board.make(mate);
                n.moves.push(mate);
                n.hashes.push(n.board.hash);
            }
        } else {
            n.vals.push(sign * cand.val);
            n.phase_confidence.push(n.confidence);
        }
        n.active = n.done.is_none();
        n
    }

    /// Локальная концентрация распределения: 1 для единственного ответа,
    /// мала при многих почти равных возможных продолжениях.
    fn concentration(cs: &[Cand], tau: f32) -> f32 {
        let peak = cs.iter().map(|c| c.val).max().unwrap_or(0);
        let mut sum = 0.0;
        let mut sq = 0.0;
        for c in cs {
            let p = ((c.val - peak) as f32 / tau).exp();
            sum += p;
            sq += p * p;
        }
        (sq / (sum * sum)).clamp(0.0, 1.0)
    }

    fn extend(&self, c: &Chain, game: &[u64], tau: f32) -> Option<Chain> {
        if !c.active {
            return Some(c.clone());
        }
        if self.stop_now() {
            return None;
        }
        let scored = self.score_moves(&c.board, game, &c.hashes, 1, true);
        if scored.iter().any(|x| x.is_none()) {
            return None;
        }
        let mut cs: Vec<Cand> = scored.into_iter().flatten().collect();
        cs.sort_by(|a, b| b.val.cmp(&a.val));
        let best = cs.first()?;
        Some(self.apply_pick(c, best, Self::concentration(&cs, tau)))
    }

    /// Первые ответы — ограниченная суперпозиция локальных волн. Амплитуды
    /// заданы энергией текущей позиции; обратной минимизации по потомкам нет.
    fn branch(&self, c: &Chain, game: &[u64], k: usize, tau: f32) -> Option<Vec<Chain>> {
        if !c.active {
            return Some(vec![c.clone()]);
        }
        if self.stop_now() {
            return None;
        }
        let scored = self.score_moves(&c.board, game, &c.hashes, 1, true);
        if scored.iter().any(|x| x.is_none()) {
            return None;
        }
        let mut cs: Vec<Cand> = scored.into_iter().flatten().collect();
        cs.sort_by(|a, b| b.val.cmp(&a.val));
        let best = cs.first()?.val;
        let certainty = Self::concentration(&cs, tau);
        let mut out = Vec::new();
        for cand in cs.iter().take(k) {
            let amplitude = ((cand.val - best) as f32 / tau).exp();
            if !out.is_empty() && amplitude < 0.02 {
                break;
            }
            let mut n = self.apply_pick(c, cand, certainty);
            n.w = amplitude;
            out.push(n);
        }
        Some(out)
    }

    fn chain_score(&self, c: &Chain, hint: Move) -> i32 {
        if c.s0.abs() >= MATE_BOUND {
            return c.s0;
        }
        if c.moves.len() == 1 && matches!(c.done, Some(End::Draw)) {
            return 0;
        }
        let weight = self.cfg.chain_pct.clamp(0, 100) as f32 / 100.0;
        // Низкочастотная часть всех наблюдённых фаз. Один далёкий скачок
        // не заменяет собой всё предшествующее развитие поля.
        let mut smooth = c.s0 as f32;
        let mut drawdown = 0.0f32;
        let mut phase_mass = 0.0f32;
        for (pair, confidence) in c.vals[1..].chunks(2).zip(c.phase_confidence[1..].chunks(2)) {
            let reliability = confidence.iter().sum::<f32>() / confidence.len() as f32;
            let phase = pair.iter().map(|&v| v as f32).sum::<f32>() / pair.len() as f32;
            drawdown += reliability * (c.s0 as f32 - phase).max(0.0).powi(2);
            phase_mass += reliability;
            smooth += 0.35 * reliability * (phase - smooth);
        }
        if matches!(c.done, Some(End::Draw)) {
            // Ничья в дальнем прогнозе не отменяет близкую потерю материала.
            smooth *= 1.0 - c.confidence;
        }
        let delta = smooth - c.s0 as f32;
        // Каждая фаза уже взвешена своей достоверностью. Это сохраняет
        // раннюю информацию и ослабляет как далёкие выигрыши, так и потери.
        let forecast = delta;
        let mut score = c.s0 as f32 + weight * forecast;
        if phase_mass > 0.0 {
            // Среднеквадратичный провал энергии: позднее восстановление
            // в одной предположительной линии не стирает ранний риск.
            score -= weight * 0.35 * (drawdown / phase_mass).sqrt();
        }
        if let Some(End::Mate { sign, ply }) = c.done {
            let bonus =
                Ps::load().mate_bonus * c.confidence / (1.0 + 0.15 * (ply as f32).powf(1.3));
            // Терминальное событие несёт больше информации, чем колебание
            // статического поля. Нулевая настройка по-прежнему выключает прогноз.
            score += weight.sqrt() * sign as f32 * bonus;
        }
        if c.mv == hint {
            score += PLAN_BONUS as f32;
        }
        score.round().clamp(-20000.0, 20000.0) as i32
    }

    fn make_lines(&self, chains: &[Chain], nvar: usize, _depth: i32, hint: Move) -> Vec<Line> {
        let ng = chains.iter().map(|c| c.grp).max().unwrap_or(0) + 1;
        let mut sum = vec![0.0f64; ng];
        let mut weights = vec![0.0f64; ng];
        let mut rep = vec![usize::MAX; ng];
        for (i, c) in chains.iter().enumerate() {
            let g = c.grp;
            sum[g] += c.w as f64 * self.chain_score(c, hint) as f64;
            weights[g] += c.w as f64;
            if rep[g] == usize::MAX || c.w > chains[rep[g]].w {
                rep[g] = i;
            }
        }
        let mut downside = vec![0.0f64; ng];
        for c in chains {
            let mean = sum[c.grp] / weights[c.grp];
            let d = (mean - self.chain_score(c, hint) as f64).max(0.0);
            downside[c.grp] += c.w as f64 * d * d;
        }
        let mut ranked = Vec::new();
        for g in 0..ng {
            if rep[g] != usize::MAX {
                // Риск нижней части спектра: опасный ответ не растворяется
                // бесследно в среднем, но никакого выбора минимума нет.
                let score = sum[g] / weights[g] - 0.5 * (downside[g] / weights[g]).sqrt();
                ranked.push((score.round() as i32, rep[g]));
            }
        }
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(chains[a.1].grp.cmp(&chains[b.1].grp)));
        ranked
            .iter()
            .take(nvar)
            .map(|&(score, i)| Line {
                score,
                depth: chains[i].moves.len() as i32,
                pv: chains[i].moves.clone(),
                local_energy: chains[i].s0,
                confidence: chains
                    .iter()
                    .filter(|c| c.grp == chains[i].grp)
                    .map(|c| c.confidence * c.w)
                    .sum::<f32>()
                    / weights[chains[i].grp] as f32,
                replies: chains.iter().filter(|c| c.grp == chains[i].grp).count(),
            })
            .collect()
    }

    /// Слабые по энергии корневые траектории сохраняют оценку, но не тратят
    /// время на далёкие фазы. Основные корневые волны сохраняются независимо
    /// от количества запрошенных строк MultiPV.
    fn focus(&self, chains: &mut [Chain], hint: Move) {
        let ranked = self.make_lines(chains, usize::MAX, 0, hint);
        if ranked.len() <= self.cfg.wave_width {
            return;
        }
        let peak = ranked[0].score;
        let keep = self.cfg.wave_width;
        for c in chains {
            let rank = ranked
                .iter()
                .position(|l| l.pv[0] == c.mv)
                .unwrap_or(usize::MAX);
            if rank >= keep && self.chain_score(c, hint) < peak - 250 {
                c.active = false;
            }
        }
    }

    fn emit_uci(&self, _depth: i32, lines: &[Line]) {
        let nodes = self.counter.load(Ordering::Relaxed);
        let el = self.start.elapsed().as_millis() as u64;
        let nps = if el > 0 { nodes * 1000 / el } else { 0 };
        for (i, l) in lines.iter().take(self.report_variants).enumerate() {
            let sc = if l.score > MATE_BOUND || l.score < -MATE_BOUND {
                let n = (MATE - l.score.abs() + 1) / 2;
                format!("mate {}", if l.score > 0 { n } else { -n })
            } else {
                format!("cp {}", l.score)
            };
            let mut pv = String::new();
            for m in l.pv.iter() {
                pv.push(' ');
                pv.push_str(&m.to_uci());
            }
            println!(
                "info depth {} multipv {} score {} nodes {} nps {} time {} pv{}",
                l.depth,
                i + 1,
                sc,
                nodes,
                nps,
                el,
                pv
            );
        }
    }

    /// Главный вход: оценить все ходы из корня и вытянуть цепочку каждого,
    /// удлиняя цепочки на полуход за итерацию, пока не кончится длина, время или не придёт stop.
    pub fn think(
        &mut self,
        root: &Board,
        game_hashes: &[u64],
        limits: &SearchLimits,
        hint: Move,
        report: Report,
    ) -> SearchResult {
        let revision = crate::params::revision();
        if revision != self.parameter_revision {
            self.cache.clear();
            self.parameter_revision = revision;
        }
        self.start = Instant::now();
        self.time_limit_ms = limits.time_ms;
        self.counter.store(0, Ordering::Relaxed);
        self.report = report;

        let mut result = SearchResult {
            best: Move::NONE,
            score: 0,
            depth: 0,
            nodes: 0,
            elapsed_ms: 0,
            lines: Vec::new(),
            continuations: Vec::new(),
        };
        let legal = root.legal_moves();
        if legal.n == 0 {
            result.score = if root.in_check() { -MATE } else { 0 };
            return result;
        }
        result.best = legal.m[0];

        if root.halfmove >= 100
            || root.insufficient()
            || game_hashes.iter().filter(|&&h| h == root.hash).count() >= 3
        {
            result.lines.push(Line {
                score: 0,
                depth: 0,
                pv: vec![result.best],
                local_energy: 0,
                confidence: 1.0,
                replies: 0,
            });
            if self.report == Report::Uci {
                self.emit_uci(0, &result.lines);
            }
            result.elapsed_ms = self.start.elapsed().as_millis() as u64;
            return result;
        }

        let mut nvar = limits.variants;
        if nvar < 1 {
            nvar = 1;
        }
        if nvar > legal.n {
            nvar = legal.n;
        }
        let mut max_depth = limits.max_depth;
        if max_depth < 1 {
            max_depth = 1;
        }
        if max_depth > MAX_DEPTH {
            max_depth = MAX_DEPTH;
        }
        let threads = if self.cfg.threads < 1 {
            1
        } else {
            self.cfg.threads
        };

        let mut game: Vec<u64> = game_hashes.to_vec();
        if game.last() != Some(&root.hash) {
            game.push(root.hash);
        }

        // Полуход 1: полностью оценённые ходы из корня; лимиты проверяются между ходами.
        let cands = self.score_moves(root, &game, &[], threads, true);
        let mut chains: Vec<Chain> = Vec::new();
        for c in cands.iter().flatten() {
            let nb = root.make(c.mv);
            let s0 = if c.term && c.val >= MATE_BOUND {
                MATE - 1
            } else {
                c.val
            };
            let done = if c.term {
                Some(if s0 >= MATE_BOUND {
                    End::Mate { sign: 1, ply: 1 }
                } else {
                    End::Draw
                })
            } else if s0 <= -MATE_BOUND {
                Some(End::Mate { sign: -1, ply: 2 })
            } else {
                None
            };
            chains.push(Chain {
                mv: c.mv,
                s0,
                moves: vec![c.mv],
                board: nb,
                hashes: vec![nb.hash],
                vals: vec![s0],
                phase_confidence: vec![1.0],
                done,
                grp: chains.len(),
                w: 1.0,
                confidence: 1.0,
                active: done.is_none(),
            });
        }

        if chains.is_empty() {
            result.elapsed_ms = self.start.elapsed().as_millis() as u64;
            return result;
        }
        let root_complete = cands.iter().all(|c| c.is_some());
        if root_complete && !root.in_check() {
            let ranked = self.make_lines(&chains, usize::MAX, 1, hint);
            let peak = ranked[0].score;
            for c in &mut chains {
                let rank = ranked.iter().position(|l| l.pv[0] == c.mv).unwrap();
                let forcing = c.board.in_check()
                    || c.mv.promo != 0
                    || root.sq[c.mv.to as usize] != 0
                    || c.mv.flag == FLAG_EP;
                if !forcing && rank >= self.cfg.wave_width && c.s0 < peak - 250 {
                    c.active = false;
                }
            }
        }
        let mut depth = 1;
        let mut best_lines = self.make_lines(&chains, nvar, depth, hint);
        if self.report == Report::Uci {
            self.emit_uci(depth, &best_lines);
        }

        // Дальше цепочки растут по полуходу за итерацию. Прерванная группа корня сохраняет последнюю полную фазу.
        loop {
            let top = best_lines[0].score;
            if top > MATE_BOUND || top < -MATE_BOUND {
                break;
            }
            if depth >= max_depth {
                break;
            }
            if !root_complete || chains.iter().all(|c| !c.active) {
                break;
            }
            if self.stop_now() {
                break;
            }
            let ps = Ps::load();
            let rk = (ps.reply_k as usize).clamp(1, 8);
            let tau = ps.reply_tau.clamp(1.0, 1000.0);
            if depth == 1 && rk > 1 {
                let br: Vec<Option<Vec<Chain>>> = par_map(chains.len(), threads, |i| {
                    self.branch(&chains[i], &game, rk, tau)
                });
                let complete = br.iter().all(|x| x.is_some());
                // Завершённые ответы одного корня публикуются вместе.
                // Дедлайн другой группы больше не уничтожает готовую работу.
                chains = br
                    .into_iter()
                    .enumerate()
                    .flat_map(|(i, bundle)| bundle.unwrap_or_else(|| vec![chains[i].clone()]))
                    .collect();
                if !complete {
                    best_lines = self.make_lines(&chains, nvar, depth + 1, hint);
                    break;
                }
            } else {
                let ext: Vec<Option<Chain>> = par_map(chains.len(), threads, |i| {
                    self.extend(&chains[i], &game, tau)
                });
                let complete = ext.iter().all(|x| x.is_some());
                let mut ready = vec![true; legal.n];
                for (i, next) in ext.iter().enumerate() {
                    if next.is_none() {
                        ready[chains[i].grp] = false;
                    }
                }
                chains = ext
                    .into_iter()
                    .enumerate()
                    .map(|(i, next)| {
                        if ready[chains[i].grp] {
                            next.unwrap()
                        } else {
                            chains[i].clone()
                        }
                    })
                    .collect();
                if !complete {
                    best_lines = self.make_lines(&chains, nvar, depth + 1, hint);
                    break;
                }
            }
            depth += 1;
            if depth == 3 {
                self.focus(&mut chains, hint);
            }
            best_lines = self.make_lines(&chains, nvar, depth, hint);
            if self.report == Report::Uci {
                self.emit_uci(depth, &best_lines);
            }
        }

        result.best = best_lines[0].pv[0];
        result.score = best_lines[0].score;
        result.depth = best_lines[0].depth;
        result.lines = best_lines;
        result.continuations = chains.iter().map(|c| c.moves.clone()).collect();
        result.nodes = self.counter.load(Ordering::Relaxed);
        result.elapsed_ms = self.start.elapsed().as_millis() as u64;
        if self.report == Report::Uci {
            self.emit_uci(result.depth, &result.lines);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_quiet_promotion_is_a_local_threat() {
        let b = Board::from_fen("7k/8/8/3R4/8/8/p7/7K b - - 0 1").unwrap();
        let without_threat = settle_value(&b, 0);
        let with_threat = settle_value(&b, 100);
        assert!(without_threat - with_threat >= 790);
        assert_eq!(with_threat, settle_value(&b.mirrored(), 100));
    }

    fn run(
        b: &Board,
        history: &[u64],
        variants: usize,
        threads: usize,
        cache_mb: usize,
    ) -> SearchResult {
        let cfg = EngineConfig {
            threads,
            cache_mb,
            ..EngineConfig::default()
        };
        let mut solver = Searcher::new(&cfg, Arc::new(AtomicBool::new(false)));
        solver.think(
            b,
            history,
            &SearchLimits {
                max_depth: 4,
                time_ms: 0,
                variants,
            },
            Move::NONE,
            Report::Quiet,
        )
    }

    #[test]
    fn completed_draw_cannot_be_escaped_by_resetting_clock() {
        let b = Board::from_fen("4k3/8/8/8/8/8/P7/R3K3 w - - 100 1").unwrap();
        let r = run(&b, &[b.hash], 1, 1, 0);
        assert_eq!(r.score, 0);
        assert_eq!(r.nodes, 0);
        let repeated = Board::startpos();
        let r = run(&repeated, &[repeated.hash; 3], 1, 1, 0);
        assert_eq!(r.score, 0);
        assert_eq!(r.nodes, 0);
    }

    #[test]
    fn multipv_threads_and_cache_do_not_change_complete_trajectory() {
        let b = Board::from_fen("4r1k1/5ppp/8/8/8/8/5PPP/R5K1 w - - 0 1").unwrap();
        let reference = run(&b, &[b.hash], 1, 1, 0);
        for (variants, threads, cache) in [(8, 1, 0), (1, 4, 1), (8, 4, 1)] {
            let r = run(&b, &[b.hash], variants, threads, cache);
            assert_eq!(r.best, reference.best);
            assert_eq!(r.score, reference.score);
            assert_eq!(r.lines[0].pv, reference.lines[0].pv);
            for line in &r.lines {
                let mut board = b;
                for &mv in &line.pv {
                    let legal = board.legal_moves();
                    assert!(legal.m[..legal.n].contains(&mv));
                    board = board.make(mv);
                }
            }
        }
    }

    #[test]
    fn speculative_draw_does_not_erase_a_near_queen_loss() {
        let b = Board::startpos();
        let mv = b.legal_moves().m[0];
        let c = Chain {
            mv,
            s0: -900,
            moves: vec![mv; 8],
            board: b,
            hashes: vec![],
            vals: vec![-900, -900, -900, -900],
            phase_confidence: vec![1.0; 4],
            done: Some(End::Draw),
            grp: 0,
            w: 1.0,
            confidence: 0.3,
            active: false,
        };
        let cfg = EngineConfig {
            threads: 1,
            cache_mb: 0,
            ..EngineConfig::default()
        };
        let s = Searcher::new(&cfg, Arc::new(AtomicBool::new(false)));
        assert!(s.chain_score(&c, Move::NONE) < -700);
        let mut terminal = c.clone();
        terminal.moves.truncate(1);
        assert_eq!(s.chain_score(&terminal, Move::NONE), 0);
    }

    #[test]
    fn immediate_stop_returns_legal_fallback_without_evaluations() {
        let b = Board::startpos();
        let cfg = EngineConfig {
            threads: 1,
            cache_mb: 0,
            ..EngineConfig::default()
        };
        let mut s = Searcher::new(&cfg, Arc::new(AtomicBool::new(true)));
        let r = s.think(
            &b,
            &[b.hash],
            &SearchLimits {
                max_depth: MAX_DEPTH,
                time_ms: 0,
                variants: 1,
            },
            Move::NONE,
            Report::Quiet,
        );
        assert!(b.legal_moves().m[..20].contains(&r.best));
        assert_eq!(r.nodes, 0);
        assert_eq!(r.depth, 0);
    }

    #[test]
    fn mate_on_fifty_move_boundary_takes_precedence() {
        let b = Board::from_fen("6k1/5ppp/8/8/8/8/8/R5K1 w - - 99 1").unwrap();
        let r = run(&b, &[b.hash], 1, 1, 0);
        assert_eq!(r.best.to_uci(), "a1a8");
        assert_eq!(r.score, MATE - 1);
    }
}
