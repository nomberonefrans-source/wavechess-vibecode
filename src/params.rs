//! Константы волновой модели в одном месте.
//!
//! Все веса заданы вручную; их сила требует проверки на позициях и в матчах.
//! Для экспериментов их можно менять на лету командой UCI
//! `setoption name W:<имя> value <число>` (см. `uci.rs`). Никакого обучения нет: во время
//! игры и между партиями ничего не меняется и нигде не сохраняется.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static REVISION: AtomicU64 = AtomicU64::new(0);

/// Версия параметров: используется для сброса устаревших волновых оценок.
pub fn revision() -> u64 {
    REVISION.load(Ordering::Acquire)
}

macro_rules! params {
    ($( $id:ident : $name:expr => $def:expr ),* $(,)?) => {
        /// Снимок всех весов (читается один раз на анализ позиции).
        #[derive(Clone, Copy, Debug)]
        pub struct Ps { $( pub $id: f32 ),* }

        pub const NAMES: &[&str] = &[ $( $name ),* ];

        const NP: usize = NAMES.len();
        static VALS: [AtomicU32; NP] = [ $( AtomicU32::new(f32::to_bits($def)) ),* ];

        impl Ps {
            #[inline]
            pub fn load() -> Ps {
                let mut i = 0usize;
                $(
                    let $id = f32::from_bits(VALS[i].load(Ordering::Relaxed));
                    i += 1;
                )*
                let _ = i;
                Ps { $( $id ),* }
            }
        }
    };
}

params! {
    // ---- поле волн
    k_control:  "Control"  => 8.0,   // контроль пространства
    k_king:     "King"     => 14.0,  // давление у королей
    k_mob:      "Mob"      => 1.0,   // охват (подвижность)
    xray_own:   "XrayOwn"  => 0.55,  // батарея
    xray_enemy: "XrayEnemy"=> 0.35,  // сквозной удар / связка
    k_flight:   "Flight"   => 30.0,  // «окно побега» короля
    k_pin:      "Pin"      => 12.0,  // связанная фигура
    k_edge:     "Edge"     => 16.0,  // загнать короля на край (эндшпиль)
    k_kprox:    "KProx"    => 8.0,   // подтянуть своего короля (эндшпиль)
    k_box:      "Box"      => 7.0,   // сократить свободные клетки короля соперника (эндшпиль)
    k_reg:      "Region"   => 4.0,   // сократить область, куда может дойти король соперника
    mate_bonus: "MateBonus"=> 4000.0, // полезность матовой фазы до скидок на дальность и достоверность
    reply_k:    "ReplyK"   => 3.0,   // сколько лучших ответов соперника берём в суперпозицию (1 — один)
    reply_tau:  "ReplyTau" => 60.0,  // «температура» амплитуд ответов, сотые пешки
    crush:      "Crush"    => 5.0,   // во сколько раз усилена тяга к мату при подавляющем перевесе
    // ---- осевшая волна (угрозы)
    counter1:   "Counter1" => 0.10,  // доля лучшей встречной угрозы
    counter2:   "Counter2" => 0.25,  // доля второй по силе встречной угрозы (вилка)
    // ---- общие потенциалы поля и перенос заряда
    relief:        "Relief"       => 16.0,
    cohesion:      "Cohesion"     => 8.0,
    transport:     "Transport"    => 1.0,
    king_shelter:  "KingShelter"  => 80.0,
    king_activity: "KingActivity" => 32.0,
}

/// Диапазоны защищают вычисления от NaN, бесконечностей и огромных амплитуд.
fn valid_value(name: &str, v: f32) -> bool {
    if !v.is_finite() || v < 0.0 {
        return false;
    }
    match name {
        "ReplyK" => (1.0..=8.0).contains(&v) && v.fract() == 0.0,
        "ReplyTau" => (1.0..=1000.0).contains(&v),
        "XrayOwn" | "XrayEnemy" | "Counter1" | "Counter2" => v <= 1.0,
        "MateBonus" => v <= 10000.0,
        "Crush" => v <= 20.0,
        _ => v <= 1000.0,
    }
}

/// Установить допустимый вес по имени. Неизвестные и некорректные значения отклоняются.
pub fn set(name: &str, v: f32) -> bool {
    for (i, n) in NAMES.iter().enumerate() {
        if n.eq_ignore_ascii_case(name) {
            if !valid_value(n, v) {
                return false;
            }
            let bits = v.to_bits();
            if VALS[i].swap(bits, Ordering::Relaxed) != bits {
                REVISION.fetch_add(1, Ordering::Release);
            }
            return true;
        }
    }
    false
}

pub fn values() -> Vec<(&'static str, f32)> {
    NAMES
        .iter()
        .enumerate()
        .map(|(i, &name)| (name, f32::from_bits(VALS[i].load(Ordering::Relaxed))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_domains_reject_nonfinite_and_unbounded_values() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, 1e20] {
            assert!(!valid_value("Control", value));
            assert!(!set("Control", value));
        }
        assert!(!valid_value("ReplyK", 0.0));
        assert!(!valid_value("ReplyK", 3.5));
        assert!(!valid_value("ReplyK", 9.0));
        assert!(valid_value("ReplyK", 8.0));
        assert!(!valid_value("ReplyTau", 0.0));
        assert!(!valid_value("XrayOwn", 1.1));
        assert!(!set("MateDef", 2.0));
        assert!(!set("MateAtt", 2.0));
        assert!(!set("MThreat", 120.0));
    }
}
