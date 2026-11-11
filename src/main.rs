//! WaveChess — шахматный движок на волновой математике.
//! Без битбордов, без обучения: поле волн, правила из геометрии, поиск цепочек ходов.

mod board;
mod cache;
mod console;
mod engine;
mod geometry;
mod notation;
mod params;
mod search;
mod selftest;
mod settings;
mod uci;
mod wave;

use std::io::IsTerminal;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetConsoleOutputCP(code_page: u32) -> i32;
    fn SetConsoleCP(code_page: u32) -> i32;
}

#[cfg(windows)]
fn setup_console() {
    // кириллица в консоли Windows
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
    }
}

#[cfg(not(windows))]
fn setup_console() {}

fn print_usage() {
    println!("WaveChess 2.2 — шахматный движок на волновой математике");
    println!();
    println!("Использование:");
    println!("  wavechess            игра в консоли (если запущен из терминала)");
    println!("  wavechess play       игра в консоли");
    println!("  wavechess uci        режим UCI для шахматных оболочек (Arena, Cute Chess ...)");
    println!("  wavechess selftest   самопроверка правил, волн и поиска");
    println!("  wavechess perft N [FEN]   подсчёт узлов для проверки правил");
    println!();
    println!("Если stdin не терминал (запуск из оболочки), включается режим UCI автоматически.");
}

fn main() {
    setup_console();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.get(0) {
        match first.as_str() {
            "uci" => {
                uci::run();
                return;
            }
            "perft" => {
                selftest::perft_cmd(&args[1..]);
                return;
            }
            "selftest" | "test" => {
                let ok = selftest::run();
                std::process::exit(if ok { 0 } else { 1 });
            }
            "play" => {
                console::run();
                return;
            }
            "help" | "--help" | "-h" => {
                print_usage();
                return;
            }
            _ => {}
        }
    }
    if std::io::stdin().is_terminal() {
        console::run();
    } else {
        uci::run();
    }
}
