//! The "pattern coach": works out *why* a hidden tile is safe (or a mine), using the same
//! techniques a human player uses, simplest first, and names the classic pattern involved.
//!
//! The [`analyzer`](crate::analyzer) already knows *which* tiles are provably safe or mines (it
//! enumerates every arrangement). This module knows nothing about the hidden mines either: it only
//! looks at the revealed numbers, exactly like a player, and records a proof for each conclusion.
//!
//! Techniques, from easiest to hardest:
//!
//! 1. **Basic** – `b1`: a number touches exactly as many hidden tiles as it needs → all mines.
//!    `b2`: a number already has all its mines → all its other neighbours are safe.
//! 2. **Pairs** – compare two neighbouring numbers. If one number's hidden tiles are a subset of
//!    the other's this is `1-1` / `1-2` (and the `p` "plus" variants). Otherwise the shared tiles
//!    can only hold so many mines (`1-2c`, and the generic *overlap* rule which also covers the
//!    hole and triangle patterns).
//! 3. **Combos** – `1-2-1` and `1-2-2-1` are two pair patterns back to back.
//! 4. **Reduction** – already-known mines are subtracted from a number before applying the above
//!    (`1-1r`, `1-2r`, …); this falls out naturally because every rule works on "mines still
//!    needed".
//! 5. **Mine counting** – use the total number of mines on the board.
//! 6. **What-if** – assume the opposite and show that the numbers then contradict each other.
//! 7. **Deep** – anything else the exhaustive analyzer can prove (long dependency chains).

use crate::server::GridConfig;
use std::{collections::BTreeMap, rc::Rc};

pub type TileId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Mine,
    Safe,
}

impl Verdict {
    pub fn opposite(self) -> Self {
        match self {
            Verdict::Mine => Verdict::Safe,
            Verdict::Safe => Verdict::Mine,
        }
    }

    pub fn phrase(self) -> &'static str {
        match self {
            Verdict::Mine => "a mine",
            Verdict::Safe => "safe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// A number touches exactly as many hidden tiles as it still needs mines.
    B1,
    /// A number already has all of its mines.
    B2,
    /// Subset pair, equal needs: the tiles only the bigger number touches are safe.
    OneOne { plus: bool },
    /// Subset pair, bigger number needs more: the tiles only it touches are all mines.
    OneTwo { plus: bool },
    /// Overlapping (non-subset) pair where one side is forced to be all mines.
    OneTwoC { plus: bool },
    /// Overlapping pair, any other conclusion (covers holes / triangles).
    Overlap,
    OneTwoOne,
    OneTwoTwoOne,
    MineCount,
    /// First line of a what-if proof.
    Assume,
    /// Last line of a what-if proof.
    WhatIf,
    /// No short pattern; proven by exhaustive analysis only.
    Deep,
}

impl Pattern {
    /// Rough difficulty, 1 (trivial) to 5 (long chain).
    pub fn level(self) -> u8 {
        match self {
            Pattern::B1 | Pattern::B2 => 1,
            Pattern::OneOne { .. }
            | Pattern::OneTwo { .. }
            | Pattern::OneTwoOne
            | Pattern::OneTwoTwoOne => 2,
            Pattern::OneTwoC { .. } | Pattern::Overlap | Pattern::MineCount => 3,
            Pattern::Assume | Pattern::WhatIf => 4,
            Pattern::Deep => 5,
        }
    }

    pub fn base_name(self) -> &'static str {
        match self {
            Pattern::B1 => "b1",
            Pattern::B2 => "b2",
            Pattern::OneOne { plus: false } => "1-1",
            Pattern::OneOne { plus: true } => "1-1p",
            Pattern::OneTwo { plus: false } => "1-2",
            Pattern::OneTwo { plus: true } => "1-2p",
            Pattern::OneTwoC { .. } => "overlap",
            Pattern::Overlap => "overlap",
            Pattern::OneTwoOne => "1-2-1",
            Pattern::OneTwoTwoOne => "1-2-2-1",
            Pattern::MineCount => "mc1",
            Pattern::Assume => "assume",
            Pattern::WhatIf => "what-if",
            Pattern::Deep => "deep chain",
        }
    }

    /// A general description of the pattern, independent of any particular board.
    pub fn blurb(self) -> &'static str {
        match self {
            Pattern::B1 => "A number that touches exactly as many hidden tiles as it still needs mines: every one of those tiles is a mine.",
            Pattern::B2 => "A number that already has all of its mines identified: every other hidden tile around it is safe.",
            Pattern::OneOne { .. } => "Two neighbouring numbers where every hidden tile of the first is also touched by the second, and both need the same number of mines. The shared tiles hold all of them, so the tiles only the second number touches are safe.",
            Pattern::OneTwo { .. } => "Two neighbouring numbers where every hidden tile of the first is also touched by the second, and the second needs more mines. The shared tiles hold the first's mines, so the extra mines the second needs must be on the tiles only it touches.",
            Pattern::OneTwoC { .. } | Pattern::Overlap => "Two numbers that share some hidden tiles, but neither one's tiles lie completely inside the other's. Ask: how many mines can the shared tiles hold at most, and at least? The smaller number sets the maximum. If the bigger number needs more than that, the rest must sit on the tiles only it sees. Once you know how many mines the shared tiles hold, the tiles only the other number sees are decided too. Many named shapes (1-2C, holes, triangles) are special cases of this.",
            Pattern::OneTwoOne => "A 2 flanked by two 1s. Each 1 leaves exactly one extra mine for the 2 on the side the 1 does not touch, so the two outer tiles are mines and the tile between them is safe.",
            Pattern::OneTwoTwoOne => "Two 2s flanked by two 1s. Each 1 pins one mine on the far side of its neighbouring 2. Those two mines fill both 2s, so every other tile around them is safe.",
            Pattern::MineCount => "Use the total number of mines on the board: once the count of unknown mines matches (or is zero) the remaining tiles are decided.",
            Pattern::Assume | Pattern::WhatIf => "Proof by contradiction: assume the opposite and follow the consequences. If the numbers can no longer all be satisfied, the assumption was wrong.",
            Pattern::Deep => "No short pattern applies. The tile is decided only by considering the whole region around it at once (a long dependency chain), or by combining it with the total mine count.",
        }
    }

    fn reducible(self) -> bool {
        matches!(
            self,
            Pattern::OneOne { .. }
                | Pattern::OneTwo { .. }
                | Pattern::OneTwoOne
                | Pattern::OneTwoTwoOne
                | Pattern::OneTwoC { .. }
                | Pattern::Overlap
        )
    }
}

/// How a tile is drawn when a step is shown on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A number doing the reasoning.
    Number,
    /// Hidden tiles being compared (shared by the numbers).
    Shared,
    /// A mine identified earlier that the numbers already count.
    KnownMine,
    /// Concluded: mine.
    Mine,
    /// Concluded: safe.
    Safe,
    /// The tile assumed in a what-if.
    Assumed,
}

#[derive(Debug, Clone)]
pub struct Step {
    pub pattern: Pattern,
    /// True if a number in this step had already-known mines subtracted first.
    pub reduced: bool,
    pub mines: Vec<TileId>,
    pub safes: Vec<TileId>,
    /// The revealed numbers involved, in the order they are labelled A, B, C, D on the board.
    pub numbers: Vec<TileId>,
    pub shared: Vec<TileId>,
    /// Known mines counted by the numbers (context only).
    pub known: Vec<TileId>,
    /// Earlier steps this one builds on.
    pub uses: Vec<usize>,
    pub text: String,
}

impl Step {
    pub fn name(&self) -> String {
        let base = self.pattern.base_name();
        if self.reduced && self.pattern.reducible() {
            match self.pattern {
                Pattern::OneOne { plus: false }
                | Pattern::OneTwo { plus: false }
                | Pattern::OneTwoOne
                | Pattern::OneTwoTwoOne => format!("{base}r"),
                _ => format!("{base} (reduced)"),
            }
        } else {
            base.to_string()
        }
    }

    pub fn label(index: usize) -> char {
        (b'A' + index as u8) as char
    }

    /// Everything to draw for this step, later entries overriding earlier ones.
    pub fn roles(&self) -> Vec<(TileId, Role)> {
        let mut out = Vec::new();
        out.extend(self.shared.iter().map(|&t| (t, Role::Shared)));
        out.extend(self.known.iter().map(|&t| (t, Role::KnownMine)));
        out.extend(self.numbers.iter().map(|&t| (t, Role::Number)));
        if self.pattern == Pattern::Assume {
            out.extend(self.mines.iter().chain(&self.safes).map(|&t| (t, Role::Assumed)));
        } else {
            out.extend(self.mines.iter().map(|&t| (t, Role::Mine)));
            out.extend(self.safes.iter().map(|&t| (t, Role::Safe)));
        }
        out
    }
}

/// A chain of steps ending with the conclusion about `cell`.
#[derive(Debug, Clone)]
pub struct Proof {
    pub cell: TileId,
    pub verdict: Verdict,
    pub steps: Vec<Step>,
}

impl Proof {
    pub fn level(&self) -> u8 {
        self.steps
            .iter()
            .map(|s| s.pattern.level())
            .max()
            .unwrap_or(1)
    }

    pub fn last(&self) -> &Step {
        self.steps.last().expect("a proof has at least one step")
    }

    pub fn headline(&self) -> String {
        self.last().name()
    }

    fn deep(cell: TileId, verdict: Verdict, numbers: Vec<TileId>) -> Self {
        let (mines, safes) = match verdict {
            Verdict::Mine => (vec![cell], vec![]),
            Verdict::Safe => (vec![], vec![cell]),
        };
        Proof {
            cell,
            verdict,
            steps: vec![Step {
                pattern: Pattern::Deep,
                reduced: false,
                mines,
                safes,
                numbers,
                shared: vec![],
                known: vec![],
                uses: vec![],
                text: format!(
                    "No short pattern proves this one. If you list every way the mines could be arranged around the numbers (and count the total mines), this tile is {} in all of them. \
                     To find it by hand, work outward from the highlighted numbers one \"if this is a mine, then…\" step at a time.",
                    verdict.phrase()
                ),
            }],
        }
    }
}

/// One thing the player could have done, with its proof.
#[derive(Debug, Clone)]
pub struct Move {
    pub verdict: Verdict,
    pub cells: Vec<TileId>,
    pub proof: Proof,
}

#[derive(Debug, Clone)]
pub struct Contradiction {
    pub text: String,
    pub numbers: Vec<TileId>,
    pub cells: Vec<TileId>,
    pub uses: Vec<usize>,
}

#[derive(Debug, Clone)]
struct NumView {
    n: TileId,
    v: u8,
    /// Mines still needed after subtracting known mines.
    r: i32,
    /// Hidden neighbours that are still undecided.
    s: Vec<TileId>,
    known: Vec<TileId>,
}

impl NumView {
    fn reduced(&self) -> bool {
        self.r != self.v as i32
    }

    /// e.g. `2 (B, acts as a 1)`
    fn tag(&self, index: usize) -> String {
        let letter = Step::label(index);
        if self.reduced() {
            format!("{} ({letter}, acts as a {})", self.v, self.r)
        } else {
            format!("{} ({letter})", self.v)
        }
    }
}

/// Appended to steps that used reduction, once, to explain the idea.
const REDUCTION_NOTE: &str = " (Reduction: mines you have already found are subtracted from a number first, so a 2 that touches one found mine behaves like a 1.)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairMode {
    Subset,
    Overlap,
}

struct PairOutcome {
    mines: Vec<TileId>,
    safes: Vec<TileId>,
    pattern: Pattern,
    subset: bool,
    shared: Vec<TileId>,
    text: String,
}

enum PairResult {
    Nothing,
    Conflict(String),
    Found(PairOutcome),
}

/// A group of undecided tiles whose numbers overlap, so they must be solved together.
struct Region {
    cells: Vec<TileId>,
    numbers: Vec<TileId>,
    /// For each number: mines it still needs, and the undecided tiles around it.
    needs: Vec<(i32, Vec<TileId>)>,
}

struct Cand {
    v: NumView,
    mine: TileId,
    safes: Vec<TileId>,
}

fn n_tiles(n: usize) -> String {
    if n == 1 {
        "1 tile".to_string()
    } else {
        format!("{n} tiles")
    }
}

fn n_mines(n: i32) -> String {
    if n == 1 {
        "1 mine".to_string()
    } else {
        format!("{n} mines")
    }
}

/// "is" / "are"
fn is_are(n: usize) -> &'static str {
    if n == 1 {
        "is"
    } else {
        "are"
    }
}

/// "it is a mine" / "they are all mines"
fn are_mines(n: usize) -> &'static str {
    if n == 1 {
        "it is a mine"
    } else {
        "they are all mines"
    }
}

/// "it is safe" / "they are all safe"
fn are_safe(n: usize) -> &'static str {
    if n == 1 {
        "it is safe"
    } else {
        "they are all safe"
    }
}

/// "the tile only B touches" / "the 3 tiles only B touches"
fn own_tiles(letter: char, n: usize) -> String {
    if n == 1 {
        format!("the tile only {letter} touches")
    } else {
        format!("the {n} tiles only {letter} touches")
    }
}

/// "1 hidden tile" / "3 hidden tiles"
fn n_hidden(n: usize) -> String {
    if n == 1 {
        "1 hidden tile".to_string()
    } else {
        format!("{n} hidden tiles")
    }
}

/// "one other tile" / "3 other tiles"
fn n_other(n: usize) -> String {
    if n == 1 {
        "one other tile".to_string()
    } else {
        format!("{n} other tiles")
    }
}

fn diff(a: &[TileId], b: &[TileId]) -> Vec<TileId> {
    a.iter().copied().filter(|t| !b.contains(t)).collect()
}

fn inter(a: &[TileId], b: &[TileId]) -> Vec<TileId> {
    a.iter().copied().filter(|t| b.contains(t)).collect()
}

fn union(parts: &[&[TileId]]) -> Vec<TileId> {
    let mut out: Vec<TileId> = parts.iter().flat_map(|p| p.iter().copied()).collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn sorted(mut v: Vec<TileId>) -> Vec<TileId> {
    v.sort_unstable();
    v
}

/// Knowledge derived purely from the revealed numbers, with a proof trail.
#[derive(Clone)]
pub struct Engine {
    width: usize,
    /// `None` disables the mine-counting rules (used by unit tests with hand-drawn boards).
    mine_total: Option<usize>,
    num: Vec<Option<u8>>,
    know: Vec<Option<(Verdict, usize)>>,
    nbrs: Rc<Vec<Vec<TileId>>>,
    frontier: Rc<Vec<TileId>>,
    pairs: Rc<Vec<(TileId, TileId)>>,
    near: Rc<Vec<Vec<TileId>>>,
    steps: Vec<Step>,
    contradiction: Option<Contradiction>,
}

impl Engine {
    /// `counts` holds, for each tile, the number it shows (`None` while hidden).
    pub fn new(config: GridConfig, counts: impl IntoIterator<Item = Option<u8>>) -> Self {
        Self::with_total(config, counts, Some(config.mine_count()))
    }

    pub fn with_total(
        config: GridConfig,
        counts: impl IntoIterator<Item = Option<u8>>,
        mine_total: Option<usize>,
    ) -> Self {
        let num: Vec<Option<u8>> = counts.into_iter().collect();
        let count = config.tile_count();
        assert_eq!(num.len(), count, "one entry per tile expected");
        let width = config.width();
        let nbrs: Vec<Vec<TileId>> = (0..count).map(|i| config.iter_adjacent(i).collect()).collect();
        let frontier: Vec<TileId> = (0..count)
            .filter(|&i| {
                matches!(num[i], Some(v) if v > 0) && nbrs[i].iter().any(|&c| num[c].is_none())
            })
            .collect();
        let mut pairs = Vec::new();
        let mut near = vec![Vec::new(); count];
        for (i, &a) in frontier.iter().enumerate() {
            for &b in &frontier[i + 1..] {
                let (ar, ac) = (a / width, a % width);
                let (br, bc) = (b / width, b % width);
                if ar.abs_diff(br) <= 2 && ac.abs_diff(bc) <= 2 {
                    pairs.push((a, b));
                    near[a].push(b);
                    near[b].push(a);
                }
            }
        }
        let mut engine = Engine {
            width,
            mine_total,
            know: vec![None; count],
            num,
            nbrs: Rc::new(nbrs),
            frontier: Rc::new(frontier),
            pairs: Rc::new(pairs),
            near: Rc::new(near),
            steps: Vec::new(),
            contradiction: None,
        };
        engine.saturate();
        engine
    }

    pub fn known(&self, cell: TileId) -> Option<Verdict> {
        self.know[cell].map(|(v, _)| v)
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Set only if the numbers are inconsistent (never happens on a real board).
    pub fn contradiction(&self) -> Option<&Contradiction> {
        self.contradiction.as_ref()
    }

    pub fn is_hidden(&self, cell: TileId) -> bool {
        self.num[cell].is_none()
    }

    // ---------------------------------------------------------------- views

    fn view(&self, n: TileId) -> NumView {
        let v = self.num[n].expect("view of a revealed tile");
        let mut s = Vec::new();
        let mut known = Vec::new();
        for &c in &self.nbrs[n] {
            if self.num[c].is_some() {
                continue;
            }
            match self.know[c] {
                None => s.push(c),
                Some((Verdict::Mine, _)) => known.push(c),
                Some((Verdict::Safe, _)) => {}
            }
        }
        NumView {
            n,
            v,
            r: v as i32 - known.len() as i32,
            s,
            known,
        }
    }

    /// Steps that established what the given numbers currently rely on.
    fn deps(&self, numbers: &[TileId]) -> Vec<usize> {
        let mut out = Vec::new();
        for &n in numbers {
            for &c in &self.nbrs[n] {
                if let Some((_, by)) = self.know[c] {
                    out.push(by);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    // ------------------------------------------------------------ inference

    fn contradict(&mut self, text: String, numbers: Vec<TileId>, cells: Vec<TileId>, uses: Vec<usize>) {
        if self.contradiction.is_none() {
            self.contradiction = Some(Contradiction {
                text,
                numbers,
                cells,
                uses,
            });
        }
    }

    /// Record a step and apply its conclusions. Returns whether anything new was learned.
    fn conclude(&mut self, mut step: Step) -> bool {
        if step.reduced && step.pattern.reducible() {
            step.text.push_str(REDUCTION_NOTE);
        }
        let id = self.steps.len();
        let mut any = false;
        let mut conflict: Option<(TileId, usize)> = None;
        for (cells, verdict) in [(&step.mines, Verdict::Mine), (&step.safes, Verdict::Safe)] {
            for &c in cells {
                match self.know[c] {
                    None => {
                        self.know[c] = Some((verdict, id));
                        any = true;
                    }
                    Some((v, _)) if v == verdict => {}
                    Some((_, by)) => conflict = Some((c, by)),
                }
            }
        }
        if !any && conflict.is_none() {
            return false;
        }
        let numbers = step.numbers.clone();
        self.steps.push(step);
        if let Some((cell, by)) = conflict {
            self.contradict(
                "That would force one tile to be both a mine and safe at the same time.".into(),
                numbers,
                vec![cell],
                vec![id, by],
            );
        }
        any
    }

    pub fn saturate(&mut self) {
        loop {
            if self.contradiction.is_some() {
                break;
            }
            if self.pass_basic()
                || self.pass_macros()
                || self.pass_pairs(PairMode::Subset)
                || self.pass_pairs(PairMode::Overlap)
                || self.pass_counting()
            {
                continue;
            }
            break;
        }
    }

    fn pass_basic(&mut self) -> bool {
        let mut progress = false;
        let frontier = self.frontier.clone();
        for &n in frontier.iter() {
            if self.contradiction.is_some() {
                break;
            }
            let v = self.view(n);
            let s = v.s.len() as i32;
            if v.r < 0 {
                let uses = self.deps(&[n]);
                self.contradict(
                    format!(
                        "The {} would then touch {} known mines, which is more than it shows.",
                        v.v,
                        v.known.len()
                    ),
                    vec![n],
                    v.known.clone(),
                    uses,
                );
                break;
            }
            if v.r > s {
                let uses = self.deps(&[n]);
                self.contradict(
                    format!(
                        "The {} would still need {} but only {} left around it. That can't work.",
                        v.v,
                        n_mines(v.r),
                        if s == 0 {
                            "no hidden tiles are".to_string()
                        } else {
                            format!("{} {}", n_tiles(s as usize), if s == 1 { "is" } else { "are" })
                        }
                    ),
                    vec![n],
                    v.known.clone(),
                    uses,
                );
                break;
            }
            if s == 0 {
                continue;
            }
            let uses = self.deps(&[n]);
            if v.r == 0 {
                let text = format!(
                    "The {} already touches {} known {}, so it is satisfied. Every other hidden tile around it is safe.",
                    v.v,
                    v.known.len(),
                    if v.known.len() == 1 { "mine" } else { "mines" }
                );
                progress |= self.conclude(Step {
                    pattern: Pattern::B2,
                    reduced: false,
                    mines: vec![],
                    safes: v.s.clone(),
                    numbers: vec![n],
                    shared: vec![],
                    known: v.known.clone(),
                    uses,
                    text,
                });
            } else if v.r == s {
                let text = if v.known.is_empty() {
                    format!(
                        "The {} touches exactly {}, so {}.",
                        v.v,
                        n_hidden(v.s.len()),
                        are_mines(v.s.len())
                    )
                } else {
                    format!(
                        "The {} already touches {} known {}, so it still needs {}. It has exactly {} left, so {}.",
                        v.v,
                        v.known.len(),
                        if v.known.len() == 1 { "mine" } else { "mines" },
                        n_mines(v.r),
                        n_hidden(v.r as usize),
                        are_mines(v.r as usize)
                    )
                };
                progress |= self.conclude(Step {
                    pattern: Pattern::B1,
                    reduced: false,
                    mines: v.s.clone(),
                    safes: vec![],
                    numbers: vec![n],
                    shared: vec![],
                    known: v.known.clone(),
                    uses,
                    text,
                });
            }
        }
        progress
    }

    fn pass_pairs(&mut self, mode: PairMode) -> bool {
        let mut progress = false;
        let pairs = self.pairs.clone();
        for &(a, b) in pairs.iter() {
            if self.contradiction.is_some() {
                break;
            }
            let (va, vb) = (self.view(a), self.view(b));
            match self.pair_outcome(&va, &vb) {
                PairResult::Nothing => {}
                PairResult::Conflict(text) => {
                    let uses = self.deps(&[a, b]);
                    self.contradict(text, vec![a, b], vec![], uses);
                }
                PairResult::Found(o) => {
                    if (mode == PairMode::Subset) != o.subset {
                        continue;
                    }
                    let mut known = union(&[&va.known, &vb.known]);
                    known.retain(|c| !o.mines.contains(c));
                    // For a subset pair the small number comes first in the text.
                    let numbers = if o.subset && diff(&va.s, &vb.s).is_empty() {
                        vec![a, b]
                    } else if o.subset {
                        vec![b, a]
                    } else {
                        vec![a, b]
                    };
                    let uses = self.deps(&[a, b]);
                    progress |= self.conclude(Step {
                        pattern: o.pattern,
                        reduced: va.reduced() || vb.reduced(),
                        mines: sorted(o.mines),
                        safes: sorted(o.safes),
                        numbers,
                        shared: o.shared,
                        known,
                        uses,
                        text: o.text,
                    });
                }
            }
        }
        progress
    }

    /// What two neighbouring numbers tell us about the tiles they touch.
    fn pair_outcome(&self, va: &NumView, vb: &NumView) -> PairResult {
        if va.s.is_empty() || vb.s.is_empty() {
            return PairResult::Nothing;
        }
        let o = inter(&va.s, &vb.s);
        if o.is_empty() {
            return PairResult::Nothing;
        }
        let x = diff(&va.s, &vb.s);
        let y = diff(&vb.s, &va.s);

        if x.is_empty() && y.is_empty() {
            return if va.r != vb.r {
                PairResult::Conflict(format!(
                    "The {} and the {} touch exactly the same hidden tiles but need different numbers of mines.",
                    va.v, vb.v
                ))
            } else {
                PairResult::Nothing
            };
        }

        if x.is_empty() || y.is_empty() {
            // Subset pair: the small number's hidden tiles are all shared.
            let a_is_small = x.is_empty();
            let (small, large, d) = if a_is_small {
                (va, vb, &y)
            } else {
                (vb, va, &x)
            };
            let m = large.r - small.r;
            if m < 0 {
                return PairResult::Conflict(format!(
                    "The {} would need fewer mines than the {}, although it touches every tile the {} does.",
                    large.v, small.v, small.v
                ));
            }
            if m > d.len() as i32 {
                return PairResult::Conflict(format!(
                    "The {} would need {} on tiles only it touches, but there are only {}.",
                    large.v,
                    n_mines(m),
                    n_tiles(d.len())
                ));
            }
            let all_mines = if m == 0 {
                false
            } else if m == d.len() as i32 {
                true
            } else {
                return PairResult::Nothing;
            };
            let plus = d.len() > 1;
            let intro = format!(
                "Every hidden tile around the {} is also around the {}. So the {} A needs must be among those shared tiles",
                small.tag(0),
                large.tag(1),
                n_mines(small.r),
            );
            let text = if !all_mines {
                format!(
                    "{intro}, which leaves none for B's {}: {}.",
                    n_other(d.len()),
                    are_safe(d.len())
                )
            } else if d.len() == 1 {
                format!(
                    "{intro}, which leaves {} for B's one other tile, so it is a mine.",
                    n_mines(m)
                )
            } else {
                format!(
                    "{intro}, which leaves {} for B's {} other tiles: one for each, so they are all mines.",
                    n_mines(m),
                    d.len()
                )
            };
            return PairResult::Found(PairOutcome {
                mines: if all_mines { d.clone() } else { vec![] },
                safes: if all_mines { vec![] } else { d.clone() },
                pattern: if all_mines {
                    Pattern::OneTwo { plus }
                } else {
                    Pattern::OneOne { plus }
                },
                subset: true,
                shared: small.s.clone(),
                text,
            });
        }

        // Overlap: x mines sit in the shared tiles.
        let (ra, rb) = (va.r, vb.r);
        let (xl, yl, ol) = (x.len() as i32, y.len() as i32, o.len() as i32);
        let xmax = ra.min(rb).min(ol);
        let xmin = 0.max(ra - xl).max(rb - yl);
        if xmin > xmax {
            return PairResult::Conflict(format!(
                "The {} and the {} could not both be satisfied by the tiles they share and their own tiles.",
                va.v, vb.v
            ));
        }
        let mut mines = Vec::new();
        let mut safes = Vec::new();
        let (mut need_max, mut need_min) = (false, false);
        let (mut a_side, mut b_side) = (None, None);
        let (la, lb) = (va.tag(0), vb.tag(1));

        // Tiles only A touches (x) and only B touches (y).
        if ra - xmin == 0 {
            safes.extend(&x);
            need_min = true;
            a_side = Some(Verdict::Safe);
        } else if ra - xmax == xl {
            mines.extend(&x);
            need_max = true;
            a_side = Some(Verdict::Mine);
        }
        if rb - xmin == 0 {
            safes.extend(&y);
            need_min = true;
            b_side = Some(Verdict::Safe);
        } else if rb - xmax == yl {
            mines.extend(&y);
            need_max = true;
            b_side = Some(Verdict::Mine);
        }
        let mut o_verdict = None;
        if xmax == 0 {
            safes.extend(&o);
            need_max = true;
            o_verdict = Some(Verdict::Safe);
        } else if xmin == ol {
            mines.extend(&o);
            need_min = true;
            o_verdict = Some(Verdict::Mine);
        }
        if mines.is_empty() && safes.is_empty() {
            return PairResult::Nothing;
        }

        // Counts first: what each number sees, so the arithmetic below is easy to follow.
        let sees = |l: &str, own: i32, r: i32| {
            format!(
                "The {l} sees {} ({} only it sees + {} shared) and needs {}.",
                format!("{} hidden {}", own + ol, if own + ol == 1 { "tile" } else { "tiles" }),
                own,
                ol,
                n_mines(r)
            )
        };
        let mut text = format!("\n{}\n{}", sees(&la, xl, ra), sees(&lb, yl, rb));
        let (mut cap_said, mut floor_said) = (false, false);
        // "The shared tiles can hold at most N, because ..."
        let mut cap = |text: &mut String| {
            if cap_said {
                return;
            }
            cap_said = true;
            let why = if xmax == ra {
                format!("A only needs {}", n_mines(ra))
            } else if xmax == rb {
                format!("B only needs {}", n_mines(rb))
            } else {
                format!("there {} only {} of them", is_are(ol as usize), ol)
            };
            *text += &format!(
                "\n\u{2022} The {} shared tiles can hold at most {}, because {why}.",
                ol,
                n_mines(xmax)
            );
        };
        // "The shared tiles must hold at least N, because ..."
        let mut floor = |text: &mut String| {
            if floor_said {
                return;
            }
            floor_said = true;
            let (letter, r, own) = if xmin == ra - xl {
                ('A', ra, xl)
            } else {
                ('B', rb, yl)
            };
            *text += &format!(
                "\n\u{2022} {letter} needs {r}, but only {own} of its tiles {} outside the shared ones, so the shared tiles must hold at least {} ({r} \u{2212} {own}).",
                is_are(own as usize),
                n_mines(xmin)
            );
        };
        let mut sides = [('B', rb, b_side, y.len()), ('A', ra, a_side, x.len())];
        // Derive the forced mines first; the safe tiles follow from them.
        sides.sort_by_key(|s| s.2 != Some(Verdict::Mine));
        for (letter, r, side, len) in sides {
            match side {
                Some(Verdict::Mine) => {
                    cap(&mut text);
                    text += &format!(
                        "\n\u{2022} {letter} needs {r} and the shared tiles give at most {xmax}, so {} must hold at least {} ({r} \u{2212} {xmax}). That is all of {}, so {}.",
                        own_tiles(letter, len),
                        r - xmax,
                        if len == 1 { "it" } else { "them" },
                        if len == 1 { "it is a mine" } else { "they are all mines" }
                    );
                    text += &format!(
                        "\n\u{2022} Check: if {} were safe, the shared tiles would have to hold {}, but they can only hold {xmax}.",
                        if len == 1 { "that tile".to_string() } else { "any one of them".to_string() },
                        xmax + 1
                    );
                }
                Some(Verdict::Safe) => {
                    floor(&mut text);
                    text += &format!(
                        "\n\u{2022} That already covers everything {letter} needs, so {} {} safe.",
                        own_tiles(letter, len),
                        is_are(len)
                    );
                }
                None => {}
            }
        }
        match o_verdict {
            Some(Verdict::Mine) => {
                floor(&mut text);
                text += "\n\u{2022} That is every shared tile, so the shared tiles are all mines.";
            }
            Some(Verdict::Safe) => {
                cap(&mut text);
                text += "\n\u{2022} So the shared tiles are safe.";
            }
            None => {}
        }
        let _ = (need_max, need_min);

        let side_mines = |side: Option<Verdict>, len: usize| {
            (side == Some(Verdict::Mine)).then_some(len)
        };
        let pattern = match side_mines(a_side, x.len()).or(side_mines(b_side, y.len())) {
            Some(len) => Pattern::OneTwoC { plus: len > 1 },
            None => Pattern::Overlap,
        };
        PairResult::Found(PairOutcome {
            mines,
            safes,
            pattern,
            subset: false,
            shared: o,
            text: text.trim_end().to_string(),
        })
    }

    /// `1-2` shapes hanging off a 2: neighbours that are 1s and whose pair with `vb` pins
    /// exactly the one tile of `vb` they do not touch.
    fn one_two_cands(&self, vb: &NumView) -> Vec<Cand> {
        let mut out = Vec::new();
        for &a in &self.near[vb.n] {
            let va = self.view(a);
            if va.r != 1 || va.s.is_empty() {
                continue;
            }
            if let PairResult::Found(o) = self.pair_outcome(&va, vb) {
                let y = diff(&vb.s, &va.s);
                if y.len() == 1 && sorted(o.mines.clone()) == y {
                    out.push(Cand {
                        v: va,
                        mine: y[0],
                        safes: o.safes,
                    });
                }
            }
        }
        out
    }

    fn pass_macros(&mut self) -> bool {
        let mut progress = false;
        let frontier = self.frontier.clone();
        for &b in frontier.iter() {
            if self.contradiction.is_some() {
                break;
            }
            let vb = self.view(b);
            if vb.r != 2 || vb.s.len() != 3 {
                continue;
            }
            let cands_b = self.one_two_cands(&vb);
            if cands_b.is_empty() {
                continue;
            }

            // 1-2-2-1
            let mut done = false;
            for &c in &self.near[b].clone() {
                if c <= b {
                    continue;
                }
                let vc = self.view(c);
                if vc.r != 2 || vc.s.len() != 3 {
                    continue;
                }
                let cands_c = self.one_two_cands(&vc);
                for ca in &cands_b {
                    for cd in &cands_c {
                        if ca.v.n == cd.v.n
                            || ca.mine == cd.mine
                            || !vc.s.contains(&ca.mine)
                            || !vb.s.contains(&cd.mine)
                        {
                            continue;
                        }
                        let mines = sorted(vec![ca.mine, cd.mine]);
                        let rest = diff(&union(&[&vb.s, &vc.s]), &mines);
                        let safes = union(&[&ca.safes, &cd.safes, &rest]);
                        let text = format!(
                            "This is the 1-2-2-1 pattern. Compare each end number with the 2 next to it (the {} with the {}, and the {} with the {}): each leaves exactly one extra mine for its 2 on the far side. \
                             Those two mines fill up both 2s, so every other hidden tile around the 2s is safe.",
                            ca.v.tag(0), vb.tag(1), cd.v.tag(3), vc.tag(2),
                        );
                        let numbers = vec![ca.v.n, b, c, cd.v.n];
                        let uses = self.deps(&numbers);
                        let reduced = ca.v.reduced() || vb.reduced() || vc.reduced() || cd.v.reduced();
                        let known = union(&[&ca.v.known, &vb.known, &vc.known, &cd.v.known]);
                        if self.conclude(Step {
                            pattern: Pattern::OneTwoTwoOne,
                            reduced,
                            mines,
                            safes,
                            numbers,
                            shared: vec![],
                            known,
                            uses,
                            text,
                        }) {
                            progress = true;
                            done = true;
                        }
                        break;
                    }
                    if done {
                        break;
                    }
                }
                if done {
                    break;
                }
            }
            if done {
                continue;
            }

            // 1-2-1
            'outer: for i in 0..cands_b.len() {
                for j in i + 1..cands_b.len() {
                    let (c1, c2) = (&cands_b[i], &cands_b[j]);
                    if c1.mine == c2.mine {
                        continue;
                    }
                    let mines = sorted(vec![c1.mine, c2.mine]);
                    let third = diff(&vb.s, &mines);
                    let safes = union(&[&c1.safes, &c2.safes, &third]);
                    let ends = if safes.len() > 1 {
                        " The tiles at the far ends, touched only by a 1, are safe too."
                    } else {
                        ""
                    };
                    let text = format!(
                        "This is the 1-2-1 pattern: the {} sits between the {} and the {}, which each need just 1 more mine. \
                         Compared with either of them, the 2 needs exactly one extra mine, on the tile that number does not touch. \
                         So the two outer tiles of the 2 are mines. That fills the 2, so its middle tile is safe.{ends}",
                        vb.tag(1),
                        c1.v.tag(0),
                        c2.v.tag(2),
                    );
                    let numbers = vec![c1.v.n, b, c2.v.n];
                    let uses = self.deps(&numbers);
                    let reduced = c1.v.reduced() || vb.reduced() || c2.v.reduced();
                    let known = union(&[&c1.v.known, &vb.known, &c2.v.known]);
                    if self.conclude(Step {
                        pattern: Pattern::OneTwoOne,
                        reduced,
                        mines,
                        safes,
                        numbers,
                        shared: vec![],
                        known,
                        uses,
                        text,
                    }) {
                        progress = true;
                    }
                    break 'outer;
                }
            }
        }
        progress
    }

    fn pass_counting(&mut self) -> bool {
        let Some(total) = self.mine_total else {
            return false;
        };
        let mut known_mines = Vec::new();
        let mut unknown = Vec::new();
        for (c, k) in self.know.iter().enumerate() {
            if self.num[c].is_some() {
                continue;
            }
            match k {
                None => unknown.push(c),
                Some((Verdict::Mine, _)) => known_mines.push(c),
                Some((Verdict::Safe, _)) => {}
            }
        }
        let remaining = total as i32 - known_mines.len() as i32;
        if remaining < 0 {
            self.contradict(
                format!("That would be more than the {total} mines on the board."),
                vec![],
                known_mines,
                vec![],
            );
            return false;
        }
        if unknown.is_empty() {
            if remaining > 0 {
                self.contradict(
                    "There would be no room left for the remaining mines.".into(),
                    vec![],
                    known_mines,
                    vec![],
                );
            }
            return false;
        }
        if remaining > unknown.len() as i32 {
            self.contradict(
                format!(
                    "There would not be enough tiles left for the remaining {}.",
                    n_mines(remaining)
                ),
                vec![],
                known_mines,
                vec![],
            );
            return false;
        }
        let (mines, safes, text) = if remaining == 0 {
            (
                vec![],
                unknown,
                format!(
                    "All {total} mines are already identified (highlighted), so every other hidden tile is safe."
                ),
            )
        } else if remaining == unknown.len() as i32 {
            (
                unknown.clone(),
                vec![],
                format!(
                    "{} still {} to be found and exactly {} undecided {} remain, so every one of them is a mine.",
                    n_mines(remaining),
                    if remaining == 1 { "has" } else { "have" },
                    unknown.len(),
                    if unknown.len() == 1 { "tile" } else { "tiles" }
                ),
            )
        } else {
            return false;
        };
        self.conclude(Step {
            pattern: Pattern::MineCount,
            reduced: false,
            mines,
            safes,
            numbers: vec![],
            shared: vec![],
            known: known_mines,
            uses: vec![],
            text,
        })
    }

    // --------------------------------------------------------------- proofs

    fn closure(&self, roots: &[usize]) -> Vec<usize> {
        let mut seen = vec![false; self.steps.len()];
        let mut stack: Vec<usize> = roots.to_vec();
        while let Some(i) = stack.pop() {
            if i >= seen.len() || seen[i] {
                continue;
            }
            seen[i] = true;
            stack.extend(self.steps[i].uses.iter().copied());
        }
        (0..seen.len()).filter(|&i| seen[i]).collect()
    }

    /// The proof for a tile the engine decided directly.
    pub fn proof(&self, cell: TileId) -> Option<Proof> {
        let (verdict, by) = self.know[cell]?;
        let steps = self
            .closure(&[by])
            .into_iter()
            .map(|i| self.steps[i].clone())
            .collect();
        Some(Proof {
            cell,
            verdict,
            steps,
        })
    }

    /// The step that decided a tile (tiles decided together share it).
    pub fn deciding_step(&self, cell: TileId) -> Option<usize> {
        self.know[cell].map(|(_, by)| by)
    }

    /// Proof by contradiction: assume the opposite of `verdict` and see whether it collapses.
    pub fn prove_by_contradiction(&self, cell: TileId, verdict: Verdict) -> Option<Proof> {
        if self.know[cell].is_some() || self.num[cell].is_some() {
            return None;
        }
        let mut trial = self.clone();
        let base_len = trial.steps.len();
        let assumed = verdict.opposite();
        let (mines, safes) = match assumed {
            Verdict::Mine => (vec![cell], vec![]),
            Verdict::Safe => (vec![], vec![cell]),
        };
        trial.conclude(Step {
            pattern: Pattern::Assume,
            reduced: false,
            mines,
            safes,
            numbers: vec![],
            shared: vec![],
            known: vec![],
            uses: vec![],
            text: format!(
                "Suppose the highlighted tile were {}. Then follow what the numbers force.",
                assumed.phrase()
            ),
        });
        trial.saturate();
        let c = trial.contradiction.clone()?;

        let mut roots = c.uses.clone();
        roots.push(base_len); // the assumption itself
        let mut steps: Vec<Step> = trial
            .closure(&roots)
            .into_iter()
            .map(|i| trial.steps[i].clone())
            .collect();
        let (mines, safes) = match verdict {
            Verdict::Mine => (vec![cell], vec![]),
            Verdict::Safe => (vec![], vec![cell]),
        };
        steps.push(Step {
            pattern: Pattern::WhatIf,
            reduced: false,
            mines,
            safes,
            numbers: c.numbers.clone(),
            shared: c.cells.clone(),
            known: vec![],
            uses: vec![],
            text: format!(
                "{} That is impossible, so the tile must be {}.",
                c.text,
                verdict.phrase()
            ),
        });
        Some(Proof {
            cell,
            verdict,
            steps,
        })
    }

    /// Best available explanation: direct proof, else what-if, else `None`.
    pub fn explain(&self, cell: TileId, verdict: Verdict) -> Option<Proof> {
        match self.know[cell] {
            Some((v, _)) if v == verdict => self.proof(cell),
            Some(_) => None,
            None => self.prove_by_contradiction(cell, verdict),
        }
    }

    /// Like [`explain`](Self::explain) but never fails: falls back to a "deep chain" note.
    pub fn explain_or_deep(&self, cell: TileId, verdict: Verdict) -> Proof {
        self.explain(cell, verdict).unwrap_or_else(|| {
            self.deep_analysis(cell, verdict).unwrap_or_else(|| {
                let numbers = self.nbrs[cell]
                    .iter()
                    .copied()
                    .filter(|&n| self.num[n].is_some_and(|v| v > 0))
                    .collect();
                Proof::deep(cell, verdict, numbers)
            })
        })
    }

    /// Undecided tiles grouped into regions that share numbers, plus the tiles no number touches.
    fn regions(&self) -> (Vec<Region>, Vec<TileId>, i32) {
        let mut views: BTreeMap<TileId, NumView> = BTreeMap::new();
        for &n in self.frontier.iter() {
            let v = self.view(n);
            if !v.s.is_empty() {
                views.insert(n, v);
            }
        }
        let mut cell_numbers: BTreeMap<TileId, Vec<TileId>> = BTreeMap::new();
        for (&n, v) in &views {
            for &c in &v.s {
                cell_numbers.entry(c).or_default().push(n);
            }
        }
        let mut known_mines = 0;
        let mut free = Vec::new();
        for c in 0..self.know.len() {
            if self.num[c].is_some() {
                continue;
            }
            match self.know[c] {
                Some((Verdict::Mine, _)) => known_mines += 1,
                None if !cell_numbers.contains_key(&c) => free.push(c),
                _ => {}
            }
        }
        let mut seen: BTreeMap<TileId, ()> = BTreeMap::new();
        let mut regions = Vec::new();
        for &start in cell_numbers.keys() {
            if seen.contains_key(&start) {
                continue;
            }
            let (mut cells, mut numbers) = (Vec::new(), Vec::new());
            let mut stack = vec![start];
            seen.insert(start, ());
            while let Some(c) = stack.pop() {
                cells.push(c);
                for &n in &cell_numbers[&c] {
                    if !numbers.contains(&n) {
                        numbers.push(n);
                        for &c2 in &views[&n].s {
                            if seen.insert(c2, ()).is_none() {
                                stack.push(c2);
                            }
                        }
                    }
                }
            }
            cells.sort_unstable();
            numbers.sort_unstable();
            let needs = numbers.iter().map(|n| (views[n].r, views[n].s.clone())).collect();
            regions.push(Region { cells, numbers, needs });
        }
        (regions, free, known_mines)
    }

    /// Enumerate every way to place mines in a region so that all its numbers are satisfied.
    /// Each arrangement is a bit mask over `region.cells`.
    fn enumerate(region: &Region) -> Option<Vec<u64>> {
        const MAX_CELLS: usize = 60;
        let n = region.cells.len();
        if n > MAX_CELLS {
            return None;
        }
        let index = |t: TileId| region.cells.binary_search(&t).expect("cell in region");
        let mut cell_cons: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut need = Vec::new();
        let mut unassigned = Vec::new();
        for (k, (r, s)) in region.needs.iter().enumerate() {
            need.push(*r);
            unassigned.push(s.len() as i32);
            for &t in s {
                cell_cons[index(t)].push(k);
            }
        }
        struct St<'a> {
            need: &'a [i32],
            cell_cons: &'a [Vec<usize>],
            mines: Vec<i32>,
            unassigned: Vec<i32>,
            mask: u64,
            out: Vec<u64>,
            overflow: bool,
        }
        fn rec(st: &mut St, i: usize, n: usize) {
            if st.overflow {
                return;
            }
            if i == n {
                st.out.push(st.mask);
                st.overflow = st.out.len() > 200_000;
                return;
            }
            for is_mine in [false, true] {
                for &c in &st.cell_cons[i] {
                    st.unassigned[c] -= 1;
                    if is_mine {
                        st.mines[c] += 1;
                    }
                }
                let ok = st.cell_cons[i]
                    .iter()
                    .all(|&c| st.mines[c] <= st.need[c] && st.mines[c] + st.unassigned[c] >= st.need[c]);
                if ok {
                    if is_mine {
                        st.mask |= 1 << i;
                    }
                    rec(st, i + 1, n);
                    st.mask &= !(1 << i);
                }
                for &c in &st.cell_cons[i] {
                    st.unassigned[c] += 1;
                    if is_mine {
                        st.mines[c] -= 1;
                    }
                }
            }
        }
        let mut st = St {
            need: &need,
            cell_cons: &cell_cons,
            mines: vec![0; need.len()],
            unassigned,
            mask: 0,
            out: Vec::new(),
            overflow: false,
        };
        rec(&mut st, 0, n);
        (!st.overflow).then_some(st.out)
    }

    /// For a tile only the exhaustive solver could decide: work out whether its own region
    /// decides it (a long chain) or whether the total mine count is needed, with real numbers.
    fn deep_analysis(&self, cell: TileId, verdict: Verdict) -> Option<Proof> {
        let (regions, free, known_mines) = self.regions();
        let mut arrangements = Vec::new();
        for r in &regions {
            arrangements.push(Self::enumerate(r)?);
        }
        let remaining = self.mine_total.map(|t| t as i32 - known_mines);
        let mine_counts: Vec<std::collections::BTreeSet<i32>> = arrangements
            .iter()
            .map(|a| a.iter().map(|m| m.count_ones() as i32).collect())
            .collect();
        let owner = regions.iter().position(|r| r.cells.binary_search(&cell).is_ok());

        // Local decision: every arrangement of the tile's own region agrees.
        if let Some(ri) = owner {
            let bit = regions[ri].cells.binary_search(&cell).unwrap();
            let all_agree = arrangements[ri]
                .iter()
                .all(|m| (m >> bit & 1 == 1) == (verdict == Verdict::Mine));
            if all_agree {
                let region = &regions[ri];
                let text = format!(
                    "No short pattern proves this one. In the highlighted region ({}) there are {} ways to place mines so that every number around it is satisfied, and this tile is {} in every one of them. \
                     To find it by hand, work through the region with \"if this tile is a mine, then…\" steps.",
                    n_tiles(region.cells.len()),
                    arrangements[ri].len(),
                    verdict.phrase()
                );
                return Some(Proof {
                    cell,
                    verdict,
                    steps: vec![Step {
                        pattern: Pattern::Deep,
                        reduced: false,
                        mines: if verdict == Verdict::Mine { vec![cell] } else { vec![] },
                        safes: if verdict == Verdict::Safe { vec![cell] } else { vec![] },
                        numbers: region.numbers.clone(),
                        shared: region.cells.iter().copied().filter(|&c| c != cell).collect(),
                        known: vec![],
                        uses: vec![],
                        text,
                    }],
                });
            }
        }

        // Otherwise the total mine count must be involved. Verify that claim ourselves: the
        // opposite verdict must be impossible once the total is respected (and ours possible).
        let remaining = remaining?;
        let free_n = free.len() as i32;
        let feasible = |cell_is_mine: bool| -> bool {
            use std::collections::BTreeSet;
            let mut others: BTreeSet<i32> = BTreeSet::from([0]);
            for (k, counts) in mine_counts.iter().enumerate() {
                if Some(k) == owner {
                    continue;
                }
                others = others
                    .iter()
                    .flat_map(|&a| counts.iter().map(move |&b| a + b))
                    .filter(|&t| t <= remaining)
                    .collect();
            }
            match owner {
                Some(ri) => {
                    let bit = regions[ri].cells.binary_search(&cell).unwrap();
                    arrangements[ri]
                        .iter()
                        .filter(|&&m| (m >> bit & 1 == 1) == cell_is_mine)
                        .any(|m| {
                            let own = m.count_ones() as i32;
                            others.iter().any(|&t| (0..=free_n).contains(&(remaining - own - t)))
                        })
                }
                None => others.iter().any(|&t| {
                    let left = remaining - t;
                    if cell_is_mine {
                        (1..=free_n).contains(&left)
                    } else {
                        (0..free_n).contains(&left)
                    }
                }),
            }
        };
        let is_mine = verdict == Verdict::Mine;
        if feasible(!is_mine) || !feasible(is_mine) {
            return None;
        }
        let lo: i32 = mine_counts.iter().map(|s| *s.iter().next().unwrap_or(&0)).sum();
        let hi: i32 = mine_counts.iter().map(|s| *s.iter().next_back().unwrap_or(&0)).sum();
        let text = format!(
            "This one needs the mine count. {} {} still hidden. The numbers force the {} next to them to hold between {} and {} of those, and the {} that touch no number take whatever is left. \
             Only arrangements using exactly {} are possible, and in every one of them this tile is {}.",
            remaining,
            if remaining == 1 { "mine is" } else { "mines are" },
            if regions.len() == 1 { "undecided region".to_string() } else { format!("{} undecided regions", regions.len()) },
            lo,
            hi,
            n_tiles(free.len()),
            n_mines(remaining),
            verdict.phrase()
        );
        let numbers: Vec<TileId> = regions.iter().flat_map(|r| r.numbers.iter().copied()).collect();
        Some(Proof {
            cell,
            verdict,
            steps: vec![Step {
                pattern: Pattern::MineCount,
                reduced: false,
                mines: if verdict == Verdict::Mine { vec![cell] } else { vec![] },
                safes: if verdict == Verdict::Safe { vec![cell] } else { vec![] },
                numbers,
                shared: vec![],
                known: vec![],
                uses: vec![],
                text,
            }],
        })
    }

    /// Group safe tiles into distinct moves, easiest first. `anchor` (e.g. the tile that was
    /// clicked) breaks ties in favour of nearby moves.
    pub fn rank_safe_moves(&self, safe_cells: &[TileId], anchor: Option<TileId>) -> Vec<Move> {
        const MAX_WHAT_IF: usize = 30;
        let mut direct: BTreeMap<usize, Vec<TileId>> = BTreeMap::new();
        let mut rest = Vec::new();
        for &c in safe_cells {
            match self.know[c] {
                Some((Verdict::Safe, by)) => direct.entry(by).or_default().push(c),
                _ => rest.push(c),
            }
        }
        let mut moves: Vec<Move> = direct
            .into_values()
            .map(|cells| Move {
                verdict: Verdict::Safe,
                proof: self.proof(cells[0]).expect("direct proof"),
                cells,
            })
            .collect();
        let mut deep = Vec::new();
        let mut tried = 0;
        for c in rest {
            let proof = if tried < MAX_WHAT_IF {
                tried += 1;
                self.prove_by_contradiction(c, Verdict::Safe)
            } else {
                None
            };
            match proof {
                Some(proof) => moves.push(Move {
                    verdict: Verdict::Safe,
                    cells: vec![c],
                    proof,
                }),
                None => deep.push(c),
            }
        }
        if !deep.is_empty() {
            moves.push(Move {
                verdict: Verdict::Safe,
                proof: self.explain_or_deep(deep[0], Verdict::Safe),
                cells: deep,
            });
        }
        let dist = |cells: &[TileId]| -> usize {
            let Some(a) = anchor else { return 0 };
            cells
                .iter()
                .map(|&c| {
                    (c / self.width).abs_diff(a / self.width) + (c % self.width).abs_diff(a % self.width)
                })
                .min()
                .unwrap_or(0)
        };
        moves.sort_by_key(|m| (m.proof.level(), m.proof.steps.len(), dist(&m.cells)));
        moves
    }
}

#[cfg(test)]
mod tests;
