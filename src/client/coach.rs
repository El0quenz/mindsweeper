//! The pattern coach UI: post-mortem explanations after a loss, click-to-inspect, and hints.

use super::{Client, Msg};
use mindsweeper::{
    analyzer::Analyzer,
    explain::{Engine, Move, Proof, Role, Step, TileId, Verdict},
    server::*,
};
use std::collections::HashMap;
use yew::{html::Scope, prelude::*};

#[derive(Clone, Copy)]
pub struct Highlight {
    pub role: Role,
    pub label: Option<char>,
}

impl Highlight {
    pub fn class(&self) -> &'static str {
        match self.role {
            Role::Number => "hl-number",
            Role::Shared => "hl-shared",
            Role::KnownMine => "hl-known-mine",
            Role::Mine => "hl-mine",
            Role::Safe => "hl-safe",
            Role::Assumed => "hl-assumed",
        }
    }
}

/// What the panel is currently explaining (also drives which list entry is highlighted).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Culprit,
    Move(usize),
    Tile(TileId),
}

pub struct Card {
    pub heading: String,
    pub lead: Option<String>,
    pub proof: Option<Proof>,
    pub note: Option<String>,
}

pub struct PostMortem {
    pub analyzer: Analyzer,
    engine: Engine,
    /// The mine that ended the game.
    culprit: Option<TileId>,
    culprit_was_decided: bool,
    /// Safe moves, nearest to the click first.
    moves: Vec<(Move, usize)>,
    wrong_flags: usize,
}

impl PostMortem {
    pub fn build(
        config: GameConfig,
        game: &impl Oracle,
        is_flagged: impl Fn(TileId) -> bool,
        last_revealed: &[TileId],
    ) -> Self {
        let grid = config.grid_config;
        let mut analyzer = Analyzer::new(config);
        analyzer.update_from(game);
        analyzer.find_safe_moves(true);
        let engine = Engine::new(grid, game.iter_adjacent_mine_counts());
        let hidden = |c: TileId| game.adjacent_mine_count(c).is_none();
        let culprit = last_revealed.iter().copied().find(|&t| game.is_mine(t));
        let safe: Vec<TileId> = (0..grid.tile_count())
            .filter(|&c| hidden(c) && analyzer.get_tile(c).is_known_safe())
            .collect();
        let wrong_flags = safe.iter().filter(|&&c| is_flagged(c)).count();
        let width = grid.width();
        let dist = |cells: &[TileId], to: Option<TileId>| -> usize {
            let Some(a) = to else { return 0 };
            cells
                .iter()
                .map(|&c| (c / width).abs_diff(a / width) + (c % width).abs_diff(a % width))
                .min()
                .unwrap_or(0)
        };
        let mut moves: Vec<(Move, usize)> = engine
            .rank_safe_moves(&safe, culprit)
            .into_iter()
            .map(|m| {
                let d = dist(&m.cells, culprit);
                (m, d)
            })
            .collect();
        // Nearest first (that is where the player was working), easier first among equals.
        moves.sort_by_key(|(m, d)| (*d / 4, m.proof.level(), m.proof.steps.len(), *d));
        PostMortem {
            culprit_was_decided: culprit.is_some_and(|c| analyzer.get_tile(c).is_known_mine()),
            analyzer,
            engine,
            culprit,
            moves,
            wrong_flags,
        }
    }

    fn card_for(&self, focus: Focus) -> Card {
        match focus {
            Focus::Culprit => match self.culprit {
                Some(c) if self.culprit_was_decided => Card {
                    heading: "Why that tile was a mine".into(),
                    lead: None,
                    proof: Some(self.engine.explain_or_deep(c, Verdict::Mine)),
                    note: None,
                },
                _ => Card {
                    heading: "That click was a guess".into(),
                    lead: None,
                    proof: None,
                    note: Some("The numbers did not decide this tile either way, so it could have been a mine or safe. Pick one of the safe moves below instead: each one has a proof.".into()),
                },
            },
            Focus::Move(i) => {
                let (m, _) = &self.moves[i];
                let n = m.cells.len();
                Card {
                    heading: format!(
                        "A safe move you had ({} safe {})",
                        n,
                        if n == 1 { "tile" } else { "tiles" }
                    ),
                    lead: None,
                    proof: Some(m.proof.clone()),
                    note: None,
                }
            }
            Focus::Tile(t) => {
                let tile = self.analyzer.get_tile(t);
                if tile.is_known_safe() {
                    Card {
                        heading: "This tile was safe".into(),
                        lead: None,
                        proof: Some(self.engine.explain_or_deep(t, Verdict::Safe)),
                        note: None,
                    }
                } else if tile.is_known_mine() {
                    Card {
                        heading: "This tile was a mine".into(),
                        lead: None,
                        proof: Some(self.engine.explain_or_deep(t, Verdict::Mine)),
                        note: None,
                    }
                } else {
                    Card {
                        heading: "This tile was undecided".into(),
                        lead: None,
                        proof: None,
                        note: Some("The numbers could not tell you whether this tile was a mine or safe, so revealing it would have been a guess. Look for tiles that have a proof instead.".into()),
                    }
                }
            }
        }
    }
}

pub struct HintState {
    pub proof: Proof,
    /// 1 = point at the numbers and name the pattern, 2 = show the whole answer.
    pub stage: u8,
}

#[derive(Default)]
pub struct Coach {
    pub post: Option<PostMortem>,
    pub hint: Option<HintState>,
    /// True once a hint or an undo was used: the game no longer counts for best times.
    pub hints_used: bool,
    /// After an undo the explanation stays on screen as a reminder until the next click.
    pub kept: bool,
    pub focus: Option<Focus>,
    pub card: Option<Card>,
    pub step: usize,
    pub open: bool,
    /// Whether the early (prerequisite) steps of a long proof are expanded.
    pub show_earlier: bool,
}

impl Coach {
    pub fn reset(&mut self) {
        *self = Coach::default();
    }

    pub fn clear_hint(&mut self) {
        if self.hint.take().is_some() | std::mem::take(&mut self.kept) {
            self.card = None;
            self.focus = None;
        }
    }

    /// The player took a losing click back: drop the post-mortem but keep its explanation
    /// visible (and drawn on the board) as a reminder.
    pub fn keep_after_undo(&mut self) {
        self.post = None;
        self.hint = None;
        self.hints_used = true;
        self.focus = None;
        self.kept = self.card.is_some();
        self.open = self.kept;
    }

    pub fn set_post_mortem(&mut self, post: PostMortem) {
        self.hint = None;
        self.open = true;
        self.post = Some(post);
        self.focus_on(Focus::Culprit);
    }

    pub fn focus_on(&mut self, focus: Focus) {
        let Some(post) = &self.post else { return };
        let card = post.card_for(focus);
        self.step = card.proof.as_ref().map_or(0, |p| p.steps.len() - 1);
        self.show_earlier = false;
        self.card = Some(card);
        self.focus = Some(focus);
    }

    pub fn select_move(&mut self, i: usize) {
        if self.post.as_ref().is_some_and(|p| i < p.moves.len()) {
            self.focus_on(Focus::Move(i));
        }
    }

    pub fn inspect(&mut self, tile: TileId) {
        let Some(post) = &self.post else { return };
        // Only hidden tiles have something to explain.
        if post.analyzer.get_tile(tile).is_revealed() {
            return;
        }
        self.open = true;
        self.focus_on(Focus::Tile(tile));
    }

    pub fn hint_stage(&self) -> Option<u8> {
        self.hint.as_ref().map(|h| h.stage)
    }

    /// Advance the hint: none → point at pattern → full answer → none.
    pub fn advance_hint(
        &mut self,
        config: GameConfig,
        game: &impl Oracle,
        anchor: Option<TileId>,
    ) {
        match self.hint_stage() {
            Some(1) => {
                if let Some(h) = &mut self.hint {
                    h.stage = 2;
                    self.step = h.proof.steps.len() - 1;
                    self.show_earlier = false;
                    self.card = Some(Card {
                        heading: "Hint".into(),
                        lead: None,
                        proof: Some(h.proof.clone()),
                        note: None,
                    });
                }
            }
            Some(_) => self.clear_hint(),
            None => {
                let grid = config.grid_config;
                let mut analyzer = Analyzer::new(config);
                analyzer.update_from(game);
                analyzer.find_safe_moves(true);
                let engine = Engine::new(grid, game.iter_adjacent_mine_counts());
                let safe: Vec<TileId> = (0..grid.tile_count())
                    .filter(|&c| {
                        game.adjacent_mine_count(c).is_none() && analyzer.get_tile(c).is_known_safe()
                    })
                    .collect();
                // Easiest pattern first; among equals prefer the one nearest the last click.
                if let Some(m) = engine.rank_safe_moves(&safe, anchor).into_iter().next() {
                    self.hints_used = true;
                    self.open = true;
                    self.focus = None;
                    self.card = None;
                    self.hint = Some(HintState {
                        proof: m.proof,
                        stage: 1,
                    });
                }
            }
        }
    }

    pub fn highlights(&self) -> HashMap<TileId, Highlight> {
        let mut map = HashMap::new();
        if let Some(h) = &self.hint {
            if h.stage == 1 {
                for step in &h.proof.steps {
                    for &n in &step.numbers {
                        map.insert(n, Highlight { role: Role::Number, label: None });
                    }
                }
                return map;
            }
        }
        let Some(proof) = self.card.as_ref().and_then(|c| c.proof.as_ref()) else {
            return map;
        };
        let step = &proof.steps[self.step.min(proof.steps.len() - 1)];
        for (tile, role) in step.roles() {
            map.insert(tile, Highlight { role, label: None });
        }
        if step.numbers.len() > 1 {
            for (i, n) in step.numbers.iter().enumerate() {
                if let Some(h) = map.get_mut(n) {
                    h.label = Some(Step::label(i));
                }
            }
        }
        map
    }
}

fn difficulty(level: u8) -> &'static str {
    match level {
        1 => "Basic",
        2 => "Pattern",
        3 => "Advanced pattern",
        4 => "What-if reasoning",
        _ => "Deep chain",
    }
}

fn legend_item(class: &'static str, text: &'static str) -> Html {
    html! {
        <span class="legend-item">
            <span class={classes!("tile", "legend-swatch", class)}></span>
            { text }
        </span>
    }
}

impl<Game: Oracle> Client<Game> {
    pub(super) fn view_coach(&self, scope: &Scope<Self>) -> Html {
        let coach = &self.coach;
        if coach.post.is_none() && coach.hint.is_none() && !coach.kept {
            return html! {};
        }
        if !coach.open {
            return html! {
                <button id="coach-tab" onclick={scope.callback(|_| Msg::ToggleCoach)}>
                    { "🎓 Coach" }
                </button>
            };
        }
        html! {
            <aside id="coach" oncontextmenu={|e: MouseEvent| e.stop_propagation()}>
                <header>
                    <strong>{ "🎓 Pattern coach" }</strong>
                    <button class="coach-close" title="Hide"
                            onclick={scope.callback(|_| Msg::ToggleCoach)}>{ "✕" }</button>
                </header>
                <div class="coach-body">
                    {
                        if let Some(post) = &coach.post {
                            self.view_post_mortem(post, scope)
                        } else if coach.kept {
                            html! {<>
                                <p class="coach-lead">
                                    { "↩ You took that click back. Here is the explanation again while you try the position once more." }
                                </p>
                                { self.view_card(scope) }
                                <p class="coach-fine">{ "Using undo means this game won't set a best time." }</p>
                            </>}
                        } else {
                            self.view_hint(scope)
                        }
                    }
                </div>
            </aside>
        }
    }

    fn view_hint(&self, scope: &Scope<Self>) -> Html {
        let coach = &self.coach;
        let Some(hint) = &coach.hint else {
            return html! {};
        };
        if hint.stage == 1 {
            let n = hint.proof.steps.iter().map(|s| s.numbers.len()).sum::<usize>();
            html! {<>
                <p class="coach-lead">
                    { "💡 There is a " }
                    <span class={classes!("badge", format!("lvl-{}", hint.proof.level()))}>
                        { hint.proof.headline() }
                    </span>
                    { " you can use. Look at the " }
                    <b>{ "blue-outlined numbers" }</b>
                    { if n > 1 { " and think about which tiles they share." } else { "." } }
                </p>
                <p class="coach-fine">
                    { "Try to work it out yourself, then press " }
                    <b>{ "Show answer" }</b>
                    { " (below the board) to see the reasoning." }
                </p>
                <p class="coach-fine">{ "Using a hint means this game won't set a best time." }</p>
            </>}
        } else {
            html! {<>
                { self.view_card(scope) }
                <p class="coach-fine">{ "Using a hint means this game won't set a best time." }</p>
            </>}
        }
    }

    fn view_post_mortem(&self, post: &PostMortem, scope: &Scope<Self>) -> Html {
        let coach = &self.coach;
        let lead = match (post.culprit, post.culprit_was_decided) {
            (Some(_), true) => html! {
                <p class="coach-lead">
                    { "You revealed a mine, but it was " }
                    <b>{ "provably" }</b>
                    { " a mine: the numbers already proved it. Here is how, and which pattern to look for next time." }
                </p>
            },
            (Some(_), false) => html! {
                <p class="coach-lead">
                    { "The tile you revealed was " }
                    <b>{ "not decided by the numbers" }</b>
                    { " (a guess), but safe moves were available." }
                </p>
            },
            _ => html! { <p class="coach-lead">{ "Game over." }</p> },
        };
        let wrong_flags = if post.wrong_flags > 0 {
            html! {
                <p class="coach-fine">
                    { format!(
                        "You also had {} wrongly placed flag{} (marked red). Click one to see why that tile was safe.",
                        post.wrong_flags,
                        if post.wrong_flags == 1 { "" } else { "s" }
                    ) }
                </p>
            }
        } else {
            html! {}
        };
        let undo = if self.undo.is_some() {
            html! {
                <button class="undo-big" onclick={scope.callback(|_| Msg::Undo)}
                        title="Restore the board to just before your last click and keep playing">
                    { "↩ Undo that click and keep playing" }
                </button>
            }
        } else {
            html! {}
        };
        let show_culprit = post.culprit.is_some();
        const SHOWN: usize = 8;
        // Basic (b1/b2) moves are obvious and numerous; list them as one row so the real
        // patterns stand out.
        let basic: Vec<usize> = (0..post.moves.len())
            .filter(|&i| post.moves[i].0.proof.level() == 1)
            .collect();
        let others: Vec<usize> = (0..post.moves.len())
            .filter(|&i| post.moves[i].0.proof.level() != 1)
            .collect();
        let basic_tiles: usize = basic.iter().map(|&i| post.moves[i].0.cells.len()).sum();
        let basic_on = matches!(coach.focus, Some(Focus::Move(i)) if basic.contains(&i));
        let move_row = |i: usize| {
            let (m, d) = &post.moves[i];
            let on = coach.focus == Some(Focus::Move(i));
            let n = m.cells.len();
            html! {
                <li class={classes!("move", on.then_some("on"))}
                    onclick={scope.callback(move |_| Msg::SelectMove(i))}>
                    <span class={classes!("badge", format!("lvl-{}", m.proof.level()))}>
                        { m.proof.headline() }
                    </span>
                    <span>{ format!("{} safe {}", n, if n == 1 { "tile" } else { "tiles" }) }</span>
                    { if *d <= 3 { html! { <span class="near">{ "near your click" }</span> } } else { html! {} } }
                </li>
            }
        };
        html! {<>
            { lead }
            { undo }
            { wrong_flags }
            {
                if show_culprit {
                    html! {
                        <div class="tabs">
                            <button class={classes!("tab", (coach.focus == Some(Focus::Culprit)).then_some("on"))}
                                    onclick={scope.callback(|_| Msg::FocusCulprit)}>
                                { "Your click" }
                            </button>
                        </div>
                    }
                } else { html! {} }
            }
            { self.view_card(scope) }
            {
                if post.moves.is_empty() {
                    html! {}
                } else {
                    html! {<>
                        <h4>{ "Safe moves you had (closest to your click first)" }</h4>
                        <ul class="moves">
                        { for others.iter().take(SHOWN).map(|&i| move_row(i)) }
                        {
                            if basic.is_empty() { html! {} } else {
                                let first = basic[0];
                                html! {
                                    <li class={classes!("move", basic_on.then_some("on"))}
                                        onclick={scope.callback(move |_| Msg::SelectMove(first))}>
                                        <span class="badge lvl-1">{ "b1 / b2" }</span>
                                        <span>{ format!("{} basic safe {}", basic_tiles, if basic_tiles == 1 { "tile" } else { "tiles" }) }</span>
                                        <span class="near">{ "obvious from one number" }</span>
                                    </li>
                                }
                            }
                        }
                        </ul>
                        {
                            if others.len() > SHOWN {
                                html! { <p class="coach-fine">{ format!("+ {} more, see the tiles shaded blue on the board.", others.len() - SHOWN) }</p> }
                            } else { html! {} }
                        }
                    </>}
                }
            }
            <p class="coach-fine">
                { "Tip: click any hidden tile on the board to see whether, and why, it was safe or a mine. Start a new game with both mouse buttons." }
            </p>
        </>}
    }

    fn view_card(&self, scope: &Scope<Self>) -> Html {
        let Some(card) = &self.coach.card else {
            return html! {};
        };
        html! {
            <div class="card">
                <h3>{ &card.heading }</h3>
                { if let Some(lead) = &card.lead { html! { <p>{ lead }</p> } } else { html! {} } }
                { if let Some(note) = &card.note { html! { <p>{ note }</p> } } else { html! {} } }
                { if let Some(proof) = &card.proof { self.view_proof(proof, scope) } else { html! {} } }
            </div>
        }
    }

    fn view_proof(&self, proof: &Proof, scope: &Scope<Self>) -> Html {
        let active = self.coach.step.min(proof.steps.len() - 1);
        let step = &proof.steps[active];
        let roles: Vec<Role> = step.roles().into_iter().map(|(_, r)| r).collect();
        let has = |r: Role| roles.contains(&r);
        let last = proof.last();
        // Long proofs: keep the pattern itself in view, tuck the prerequisites away.
        const VISIBLE: usize = 3;
        let early = proof.steps.len().saturating_sub(VISIBLE);
        let expanded = self.coach.show_earlier || active < early;
        html! {<>
            <div class="badge-row">
                <span class={classes!("badge", "big", format!("lvl-{}", proof.level()))}>{ proof.headline() }</span>
                <span class={classes!("verdict", match proof.verdict { Verdict::Mine => "is-mine", Verdict::Safe => "is-safe" })}>
                    { match proof.verdict { Verdict::Mine => "mine", Verdict::Safe => "safe" } }
                </span>
                <span class="difficulty">{ difficulty(proof.level()) }</span>
            </div>
            <p class="blurb">{ last.pattern.blurb() }</p>
            <p class="coach-fine">
                {
                    if proof.steps.len() > 1 {
                        "How the numbers show it (click a step to see it on the board):"
                    } else {
                        "How the numbers show it:"
                    }
                }
            </p>
            {
                if early > 0 {
                    html! {
                        <button class="earlier"
                                onclick={scope.callback(|_| Msg::ToggleEarlier)}>
                            {
                                if expanded {
                                    format!("▾ Hide the {early} earlier steps")
                                } else {
                                    format!("▸ Show the {early} earlier steps (they find the mines the numbers rely on)")
                                }
                            }
                        </button>
                    }
                } else { html! {} }
            }
            <ol class="steps" style={format!("counter-reset: step {};", if expanded { 0 } else { early })}>
            { for proof.steps.iter().enumerate()
                .filter(|(i, _)| expanded || *i >= early)
                .map(|(i, s)| html! {
                    <li class={classes!("step", (i == active).then_some("on"))}
                        onclick={scope.callback(move |_| Msg::SelectStep(i))}>
                        <span class="badge small">{ s.name() }</span>
                        { " " }
                        { &s.text }
                    </li>
                }) }
            </ol>
            <div class="legend">
                { if has(Role::Number) { legend_item("hl-number", "numbers used") } else { html! {} } }
                { if has(Role::Shared) { legend_item("hl-shared", "tiles compared") } else { html! {} } }
                { if has(Role::KnownMine) { legend_item("hl-known-mine", "mines already found") } else { html! {} } }
                { if has(Role::Mine) { legend_item("hl-mine", "mines") } else { html! {} } }
                { if has(Role::Safe) { legend_item("hl-safe", "safe") } else { html! {} } }
                { if has(Role::Assumed) { legend_item("hl-assumed", "assumed") } else { html! {} } }
            </div>
        </>}
    }
}
