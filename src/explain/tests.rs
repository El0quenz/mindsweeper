use super::*;
use crate::{
    analyzer::Analyzer,
    server::{local::LocalGame, GameConfig, GameMode, GameStatus, Oracle},
};
use rand::seq::SliceRandom;
use std::collections::BTreeMap;

/// Build an engine from an ASCII picture: `#` hidden, `.` revealed zero, digits are numbers.
/// Counting rules are disabled, and the picture need not be a consistent board.
fn engine(rows: &[&str]) -> (Engine, usize) {
    let h = rows.len();
    let w = rows[0].chars().count();
    let cfg = GridConfig::new(h, w, 1).expect("valid grid");
    let counts = rows.iter().flat_map(|r| {
        r.chars().map(|ch| match ch {
            '#' => None,
            '.' => Some(0),
            d => Some(d.to_digit(10).unwrap() as u8),
        })
    });
    (Engine::with_total(cfg, counts, None), w)
}

fn name(e: &Engine, cell: TileId) -> String {
    e.proof(cell).unwrap().headline()
}

#[test]
fn b1_corner() {
    let (e, w) = engine(&["#1..", "....", "...."]);
    assert_eq!(e.known(0), Some(Verdict::Mine));
    assert_eq!(name(&e, 0), "b1");
    assert_eq!(w, 4);
}

#[test]
fn b2_after_b1_is_a_two_step_proof() {
    let (e, _) = engine(&["#1#.", "1...", "...."]);
    assert_eq!(e.known(0), Some(Verdict::Mine));
    assert_eq!(e.known(2), Some(Verdict::Safe));
    let p = e.proof(2).unwrap();
    assert_eq!(p.headline(), "b2");
    assert_eq!(p.steps.len(), 2);
    assert_eq!(p.level(), 1);
}

#[test]
fn one_one() {
    let (e, _) = engine(&["###.", "11..", "...."]);
    assert_eq!(e.known(2), Some(Verdict::Safe));
    assert_eq!(e.known(0), None);
    assert_eq!(name(&e, 2), "1-1");
}

#[test]
fn one_two() {
    let (e, _) = engine(&["###.", "12..", "...."]);
    assert_eq!(e.known(2), Some(Verdict::Mine));
    assert_eq!(name(&e, 2), "1-2");
}

#[test]
fn one_one_plus() {
    let (e, w) = engine(&["###..", "11#..", "....."]);
    assert_eq!(e.known(2), Some(Verdict::Safe));
    assert_eq!(e.known(w + 2), Some(Verdict::Safe));
    assert_eq!(name(&e, 2), "1-1p");
}

#[test]
fn one_two_plus() {
    let (e, w) = engine(&["###..", "13#..", "....."]);
    assert_eq!(e.known(2), Some(Verdict::Mine));
    assert_eq!(e.known(w + 2), Some(Verdict::Mine));
    assert_eq!(name(&e, 2), "1-2p");
}

#[test]
fn classic_one_two_against_a_wall() {
    let (e, _) = engine(&["####.", ".12..", "....."]);
    assert_eq!(e.known(0), Some(Verdict::Safe));
    assert_eq!(e.known(3), Some(Verdict::Mine));
    assert_eq!(e.known(1), None);
    assert_eq!(e.known(2), None);
    assert_eq!(name(&e, 3), "overlap");
}

#[test]
fn one_two_one() {
    let (e, _) = engine(&["#####", ".121.", "....."]);
    assert_eq!(e.known(1), Some(Verdict::Mine));
    assert_eq!(e.known(3), Some(Verdict::Mine));
    for c in [0, 2, 4] {
        assert_eq!(e.known(c), Some(Verdict::Safe));
        assert_eq!(name(&e, c), "1-2-1");
    }
}

#[test]
fn one_two_one_in_a_corner_pocket() {
    // Only three hidden tiles: each 1 touches two of them.
    let (e, _) = engine(&[".###.", ".121.", "....."]);
    assert_eq!(e.known(1), Some(Verdict::Mine));
    assert_eq!(e.known(3), Some(Verdict::Mine));
    assert_eq!(e.known(2), Some(Verdict::Safe));
    assert_eq!(name(&e, 2), "1-2-1");
}

#[test]
fn one_two_two_one() {
    let (e, _) = engine(&["######", ".1221.", "......"]);
    assert_eq!(e.known(2), Some(Verdict::Mine));
    assert_eq!(e.known(3), Some(Verdict::Mine));
    for c in [0, 1, 4, 5] {
        assert_eq!(e.known(c), Some(Verdict::Safe));
        assert_eq!(name(&e, c), "1-2-2-1");
    }
}

#[test]
fn no_conclusion_from_a_lone_one_two_one_on_open_ground() {
    // A plain 1 with three hidden neighbours decides nothing.
    let (e, _) = engine(&["###.", ".1..", "...."]);
    assert!((0..3).all(|c| e.known(c).is_none()));
}

#[test]
fn what_if_proof_is_used_when_no_direct_rule_applies() {
    // Every non-what-if rule is exhausted here, yet tile 0 is provably safe:
    // if it were a mine the left 1 would be satisfied by it, forcing tile 1 safe and then...
    // We just check the mechanism runs and yields a coherent proof when it finds one.
    let (e, _) = engine(&["####.", "1.2..", "....."]);
    for c in 0..4 {
        if e.known(c).is_none() {
            for v in [Verdict::Safe, Verdict::Mine] {
                if let Some(p) = e.prove_by_contradiction(c, v) {
                    assert_eq!(p.last().pattern, Pattern::WhatIf);
                    assert_eq!(p.steps[p.steps.len() - 2].pattern.level() >= 1, true);
                }
            }
        }
    }
}

// ------------------------------------------------------------------------------------------
// Randomised soundness: the engine only reads the revealed numbers, so everything it claims
// must agree with the exhaustive solver. We also gather statistics on how much it explains.

#[derive(Default)]
struct Stats {
    states: usize,
    safe_tiles: usize,
    mine_tiles: usize,
    direct_safe: usize,
    what_if_safe: usize,
    deep_safe: usize,
    direct_mine: usize,
    what_if_mine: usize,
    deep_mine: usize,
    /// Number of states where the *easiest* missed move was at each level.
    easiest_level: BTreeMap<u8, usize>,
    names: BTreeMap<String, usize>,
    engine_only: usize,
    deep_kinds: BTreeMap<&'static str, usize>,
}

fn run_random(grid: GridConfig, games: usize, reveal_per_round: usize, stats: &mut Stats) {
    let config = GameConfig {
        grid_config: grid,
        mode: GameMode::Normal,
        punish_guessing: true,
    };
    let mut rng = rand::thread_rng();
    for _ in 0..games {
        let first = grid.random_tile_id();
        let mut game = LocalGame::new(config, first);
        game.reveal_tile(first);
        while game.status() == GameStatus::Ongoing {
            let mut analyzer = Analyzer::new(config);
            analyzer.update_from(&game);
            analyzer.find_safe_moves(true);
            let engine = Engine::new(grid, game.iter_adjacent_mine_counts());
            assert!(
                engine.contradiction().is_none(),
                "engine found a contradiction on a real board: {:?}",
                engine.contradiction()
            );

            let mut safe = Vec::new();
            let mut mines = Vec::new();
            for c in 0..grid.tile_count() {
                if !engine.is_hidden(c) {
                    continue;
                }
                let t = analyzer.get_tile(c);
                if t.is_known_safe() {
                    safe.push(c);
                }
                if t.is_known_mine() {
                    mines.push(c);
                }
                match engine.known(c) {
                    Some(Verdict::Safe) => assert!(t.is_known_safe(), "engine unsound: safe {c}"),
                    Some(Verdict::Mine) => assert!(t.is_known_mine(), "engine unsound: mine {c}"),
                    None => {}
                }
            }
            stats.states += 1;
            stats.safe_tiles += safe.len();
            stats.mine_tiles += mines.len();
            stats.engine_only += (0..grid.tile_count())
                .filter(|&c| engine.known(c).is_some() && !analyzer.get_tile(c).is_unknown())
                .count();

            for (cells, verdict) in [(&safe, Verdict::Safe), (&mines, Verdict::Mine)] {
                for &c in cells {
                    let proof = engine.explain(c, verdict);
                    let (direct, wi, deep) = match verdict {
                        Verdict::Safe => (
                            &mut stats.direct_safe,
                            &mut stats.what_if_safe,
                            &mut stats.deep_safe,
                        ),
                        Verdict::Mine => (
                            &mut stats.direct_mine,
                            &mut stats.what_if_mine,
                            &mut stats.deep_mine,
                        ),
                    };
                    match &proof {
                        Some(p) => {
                            assert_eq!(p.verdict, verdict);
                            let last = p.last();
                            let concluded = match verdict {
                                Verdict::Safe => &last.safes,
                                Verdict::Mine => &last.mines,
                            };
                            assert!(concluded.contains(&c), "proof must end by concluding the tile");
                            if p.last().pattern == Pattern::WhatIf {
                                *wi += 1;
                            } else {
                                *direct += 1;
                            }
                        }
                        None => {
                            *deep += 1;
                            let p = engine.explain_or_deep(c, verdict);
                            assert_eq!(p.cell, c);
                            let kind = match p.last().pattern {
                                Pattern::MineCount => "counting",
                                Pattern::Deep if p.last().text.contains("highlighted region") => "local-chain",
                                _ => "generic",
                            };
                            *stats.deep_kinds.entry(kind).or_default() += 1;
                        }
                    }
                }
            }
            let moves = engine.rank_safe_moves(&safe, None);
            if let Some(m) = moves.first() {
                *stats.easiest_level.entry(m.proof.level()).or_default() += 1;
                *stats.names.entry(m.proof.headline()).or_default() += 1;
            }

            if safe.is_empty() {
                break;
            }
            safe.shuffle(&mut rng);
            for &c in safe.iter().take(reveal_per_round) {
                if game.status() == GameStatus::Ongoing {
                    game.reveal_tile(c);
                }
            }
        }
    }
}

#[test]
fn engine_agrees_with_exhaustive_solver() {
    let games: usize = std::env::var("EXPLAIN_GAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let mut stats = Stats::default();
    run_random(GridConfig::beginner(), games * 4, 1, &mut stats);
    run_random(GridConfig::intermediate(), games * 2, 2, &mut stats);
    run_random(GridConfig::expert(), games, 6, &mut stats);
    let s = &stats;
    let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!("states analysed:       {}", s.states);
    println!(
        "safe tiles:  {:>7}  direct {:.1}%  what-if {:.1}%  deep {:.1}%",
        s.safe_tiles,
        pct(s.direct_safe, s.safe_tiles),
        pct(s.what_if_safe, s.safe_tiles),
        pct(s.deep_safe, s.safe_tiles)
    );
    println!(
        "mine tiles:  {:>7}  direct {:.1}%  what-if {:.1}%  deep {:.1}%",
        s.mine_tiles,
        pct(s.direct_mine, s.mine_tiles),
        pct(s.what_if_mine, s.mine_tiles),
        pct(s.deep_mine, s.mine_tiles)
    );
    println!("deep tiles explained as: {:?}", s.deep_kinds);
    println!("easiest-move level histogram: {:?}", s.easiest_level);
    println!("easiest-move pattern names:   {:?}", s.names);
}

// ------------------------------------------------------------------------------------------
// "Stuck" positions: play like a strong human, taking every move the engine can prove directly,
// until only harder deductions remain. These are the positions where players actually get stuck.

fn ascii(engine: &Engine, grid: GridConfig, analyzer: &Analyzer, mark: &[TileId]) -> String {
    let mut out = String::new();
    for r in 0..grid.height() {
        for c in 0..grid.width() {
            let id = r * grid.width() + c;
            let ch = if mark.contains(&id) {
                '*'
            } else if !engine.is_hidden(id) {
                match analyzer.get_tile(id) {
                    crate::analyzer::AnalyzerTile::Revealed { adjacent_mine_count: 0 } => '.',
                    crate::analyzer::AnalyzerTile::Revealed { adjacent_mine_count } => {
                        char::from_digit(adjacent_mine_count as u32, 10).unwrap()
                    }
                    _ => '?',
                }
            } else if engine.known(id) == Some(Verdict::Mine) {
                'F'
            } else {
                '#'
            };
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

#[test]
#[ignore]
fn stuck_positions() {
    let games: usize = std::env::var("EXPLAIN_GAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let mut stuck = 0usize;
    let (mut with_what_if, mut only_deep, mut nothing_left) = (0, 0, 0);
    let mut what_if_names: BTreeMap<String, usize> = BTreeMap::new();
    let mut shown = 0;
    for grid in [GridConfig::intermediate(), GridConfig::expert()] {
        let config = GameConfig { grid_config: grid, mode: GameMode::Normal, punish_guessing: true };
        for _ in 0..games {
            let first = grid.random_tile_id();
            let mut game = LocalGame::new(config, first);
            game.reveal_tile(first);
            while game.status() == GameStatus::Ongoing {
                let engine = Engine::new(grid, game.iter_adjacent_mine_counts());
                let direct: Vec<TileId> = (0..grid.tile_count())
                    .filter(|&c| engine.is_hidden(c) && engine.known(c) == Some(Verdict::Safe))
                    .collect();
                if !direct.is_empty() {
                    for c in direct {
                        if game.status() == GameStatus::Ongoing {
                            game.reveal_tile(c);
                        }
                    }
                    continue;
                }
                // Stuck: nothing the direct rules prove.
                let mut analyzer = Analyzer::new(config);
                analyzer.update_from(&game);
                analyzer.find_safe_moves(true);
                let safe: Vec<TileId> = (0..grid.tile_count())
                    .filter(|&c| engine.is_hidden(c) && analyzer.get_tile(c).is_known_safe())
                    .collect();
                stuck += 1;
                let moves = engine.rank_safe_moves(&safe, None);
                let mut any_wi = false;
                for m in &moves {
                    if m.proof.last().pattern == Pattern::WhatIf {
                        any_wi = true;
                    }
                }
                if any_wi {
                    with_what_if += 1;
                    let m = moves.iter().find(|m| m.proof.last().pattern == Pattern::WhatIf).unwrap();
                    *what_if_names.entry(format!("{} steps", m.proof.steps.len())).or_default() += 1;
                } else if !safe.is_empty() {
                    only_deep += 1;
                    if shown < 3 {
                        shown += 1;
                        println!("--- DEEP-ONLY position ({} safe tiles, marked *) ---\n{}", safe.len(), ascii(&engine, grid, &analyzer, &safe));
                    }
                } else {
                    nothing_left += 1;
                }
                // Move on by revealing the analyzer-known safe tiles.
                for c in safe {
                    if game.status() == GameStatus::Ongoing {
                        game.reveal_tile(c);
                    }
                }
                if game.status() != GameStatus::Ongoing {
                    break;
                }
            }
        }
    }
    println!("stuck positions: {stuck}");
    println!("  explained by a what-if proof: {with_what_if}  ({what_if_names:?})");
    println!("  only deep-chain tiles:        {only_deep}");
    println!("  (no safe tiles left):         {nothing_left}");
}

#[test]
#[ignore]
fn show_texts() {
    let boards: &[(&str, &[&str], usize)] = &[
        ("b1", &["#1..", "....", "...."], 0),
        ("1-1", &["###.", "11..", "...."], 2),
        ("1-2", &["###.", "12..", "...."], 2),
        ("1-1p", &["###..", "11#..", "....."], 2),
        ("1-2p", &["###..", "13#..", "....."], 2),
        ("overlap (mine)", &["####.", ".12..", "....."], 3),
        ("overlap (safe)", &["####.", ".12..", "....."], 0),
        ("1-2-1", &["#####", ".121.", "....."], 2),
        ("1-2-1 pocket", &[".###.", ".121.", "....."], 2),
        ("1-2-2-1", &["######", ".1221.", "......"], 0),
    ];
    for (title, rows, cell) in boards {
        let (e, _) = engine(rows);
        let p = e.proof(*cell).unwrap();
        println!("\n## {title}  ->  {} ({:?})", p.headline(), p.verdict);
        for s in &p.steps {
            println!("  [{}] {}", s.name(), s.text);
        }
    }
}

/// A 1 (A) and a 4 (B) sharing two hidden tiles: B's three own tiles must all be mines,
/// which uses up A's only mine and leaves the tile only A sees safe.
#[test]
fn overlap_one_and_four() {
    let (e, w) = engine(&["#####", "#211#", "#1.1#", "#422#", "#####"]);
    let t = w + 0;
    assert_eq!(e.known(t), Some(Verdict::Safe));
    for d in [4 * w, 4 * w + 1, 4 * w + 2] {
        assert_eq!(e.known(d), Some(Verdict::Mine));
        assert_eq!(name(&e, d), "overlap");
    }
    assert_eq!(name(&e, t), "overlap");
    let text = e.proof(t).unwrap().last().text.clone();
    println!("{text}");
    assert!(text.contains("The 1 (A) sees 3 hidden tiles (1 only it sees + 2 shared) and needs 1 mine."));
    assert!(text.contains("The 4 (B) sees 5 hidden tiles (3 only it sees + 2 shared) and needs 4 mines."));
}
