/// Postflop single-size auto search.
///
/// For a given turn subgame, find one bet size and one raise size per player
/// for the turn and (optionally) the river by coordinate search.  River
/// multi-size fallback comes from the shared `BetConfig::sizes`.

use crate::cfr::{SubgameConfig, SubgameSolver};
use crate::export::SolveResult;
use crate::game::{BetConfig, BetSize, Street, IP, OOP};

fn street_label(s: Street) -> &'static str {
    match s {
        Street::Preflop => "preflop",
        Street::Flop => "flop",
        Street::Turn => "turn",
        Street::River => "river",
    }
}

/// One candidate size set for a single street.
#[derive(Debug, Clone, Copy)]
pub struct TurnSizeSet {
    pub oop_bet: BetSize,
    pub oop_raise: Option<BetSize>,
    pub ip_bet: BetSize,
    pub ip_raise: Option<BetSize>,
}

fn bet_size_label(s: BetSize) -> String {
    match s {
        BetSize::Frac(n, 100) => format!("{}%", n),
        BetSize::Frac(n, 1) => format!("{}x", n),
        BetSize::Frac(n, d) => format!("{}/{}", n, d),
        BetSize::Bb(n) => format!("{}bb", n),
    }
}

impl TurnSizeSet {
    /// A short human-readable description, e.g. "OOP 50/100, IP 75/150".
    pub fn display(&self) -> String {
        let oop_raise = self
            .oop_raise
            .map_or("-".to_string(), bet_size_label);
        let ip_raise = self
            .ip_raise
            .map_or("-".to_string(), bet_size_label);
        format!(
            "OOP bet {} / raise {}, IP bet {} / raise {}",
            bet_size_label(self.oop_bet),
            oop_raise,
            bet_size_label(self.ip_bet),
            ip_raise
        )
    }
}

/// Candidate sizes for both the turn and the river.
/// A street is "active" if any of its four candidate lists is non-empty.
#[derive(Debug, Clone)]
pub struct PostflopSizeSet {
    pub turn: Option<TurnSizeSet>,
    pub river: Option<TurnSizeSet>,
}

impl PostflopSizeSet {
    /// Human-readable description of the chosen sizes.
    pub fn display(&self) -> String {
        let turn = self
            .turn
            .as_ref()
            .map_or("turn=multi-size".to_string(), |s| format!("turn=[{}]", s.display()));
        let river = self
            .river
            .as_ref()
            .map_or("river=multi-size".to_string(), |s| format!("river=[{}]", s.display()));
        format!("{}, {}", turn, river)
    }
}

/// Progress update emitted during the auto search.
#[derive(Debug, Clone)]
pub struct SearchProgress {
    pub phase: &'static str,
    pub street: &'static str,
    pub round: u32,
    pub total_rounds: u32,
    pub role: &'static str,
    pub candidate: usize,
    pub total_candidates: usize,
    pub iter: u32,
    pub total_iter: u32,
    pub expl_pct: f32,
}

/// Candidate pools for one street.
#[derive(Debug, Clone)]
pub struct TurnSizeCandidates {
    pub oop_bet: Vec<BetSize>,
    pub oop_raise: Vec<BetSize>,
    pub ip_bet: Vec<BetSize>,
    pub ip_raise: Vec<BetSize>,
}

impl TurnSizeCandidates {
    fn role(&self, idx: usize) -> &[BetSize] {
        match idx {
            0 => &self.oop_bet,
            1 => &self.oop_raise,
            2 => &self.ip_bet,
            3 => &self.ip_raise,
            _ => panic!("invalid role idx"),
        }
    }

    fn has_any(&self) -> bool {
        !self.oop_bet.is_empty()
            || !self.oop_raise.is_empty()
            || !self.ip_bet.is_empty()
            || !self.ip_raise.is_empty()
    }
}

/// Candidate pools for both the turn and the river.
#[derive(Debug, Clone)]
pub struct PostflopSizeCandidates {
    pub turn: TurnSizeCandidates,
    pub river: TurnSizeCandidates,
}

impl PostflopSizeCandidates {
    /// Map a flat 0..7 role index to a street and an inner 0..3 role index.
    fn street_and_role(idx: usize) -> (Street, usize) {
        match idx {
            0..=3 => (Street::Turn, idx),
            4..=7 => (Street::River, idx - 4),
            _ => panic!("invalid postflop role idx"),
        }
    }

    fn street_candidates(&self, street: Street) -> &TurnSizeCandidates {
        match street {
            Street::Turn => &self.turn,
            Street::River => &self.river,
            _ => panic!("invalid street for sizing"),
        }
    }

    /// Human-readable role name for the flat 0..7 index.
    fn role_name(idx: usize) -> &'static str {
        let (street, inner) = Self::street_and_role(idx);
        match street {
            Street::Turn => match inner {
                0 => "turn OOP bet",
                1 => "turn OOP raise",
                2 => "turn IP bet",
                3 => "turn IP raise",
                _ => unreachable!(),
            },
            Street::River => match inner {
                0 => "river OOP bet",
                1 => "river OOP raise",
                2 => "river IP bet",
                3 => "river IP raise",
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    fn has_any(&self) -> bool {
        self.turn.has_any() || self.river.has_any()
    }
}

/// Build the per-player `player_sizes` override for a single street.
fn build_street_override(street: Street, set: &TurnSizeSet, bc: &mut BetConfig) {
    fn turn_depths(bet: BetSize, raise: Option<BetSize>) -> Vec<Vec<BetSize>> {
        let mut v = vec![vec![bet]];
        if let Some(r) = raise {
            v.push(vec![r]);
        } else {
            v.push(vec![]);
        }
        v
    }

    let turn_idx = street.index();

    let mut oop_streets = bc.player_sizes[OOP as usize]
        .take()
        .unwrap_or([Vec::new(), Vec::new(), Vec::new(), Vec::new()]);
    oop_streets[turn_idx] = turn_depths(set.oop_bet, set.oop_raise);
    bc.player_sizes[OOP as usize] = Some(oop_streets);

    let mut ip_streets = bc.player_sizes[IP as usize]
        .take()
        .unwrap_or([Vec::new(), Vec::new(), Vec::new(), Vec::new()]);
    ip_streets[turn_idx] = turn_depths(set.ip_bet, set.ip_raise);
    bc.player_sizes[IP as usize] = Some(ip_streets);
}

/// Build a `BetConfig` from a base config and chosen postflop sizes.
/// Streets that are `None` use the base config's shared `sizes` instead.
pub fn build_postflop_bet_config(base: &BetConfig, set: &PostflopSizeSet) -> BetConfig {
    let mut bc = base.clone();
    if let Some(turn) = set.turn {
        build_street_override(Street::Turn, &turn, &mut bc);
    }
    if let Some(river) = set.river {
        build_street_override(Street::River, &river, &mut bc);
    }
    bc
}

/// Score a solved candidate.  Higher is better.
/// We use OOP EV minus half the exploitability (in chips).  This rewards high
/// EV while penalising candidates that have not converged.
fn score_solver(solver: &SubgameSolver) -> f32 {
    let (oop_ev, _) = solver.overall_ev();
    let expl = solver.exploitability().max(0.0);
    oop_ev - 0.5 * expl
}

/// Run one solve with the given size set and a (possibly reduced) iteration
/// budget, calling `progress` as the solver reports.
fn evaluate_candidate<F>(
    base: &SubgameConfig,
    bet_config: BetConfig,
    algorithm: &str,
    qre_lambda: f32,
    qre_damping: f32,
    qre_anneal: bool,
    iterations: u32,
    early_stop_pct: f32,
    skip_cum_strategy: bool,
    mut progress: F,
) -> (SubgameSolver, f32)
where
    F: FnMut(u32, u32, f32),
{
    let mut config = base.clone();
    config.bet_config = Some(std::sync::Arc::new(bet_config));
    config.iterations = iterations;
    config.early_stop_pct = early_stop_pct;
    config.early_stop_patience = 2;
    config.skip_cum_strategy = skip_cum_strategy;

    let mut solver = SubgameSolver::new(config);
    solver.algorithm = algorithm.to_string();
    let report_every = (iterations / 20).max(1);

    let progress_cb = |iter: u32, s: &SubgameSolver| {
        let expl = s.last_exploitability_pct.unwrap_or(s.exploitability_pct());
        progress(iter, iterations, expl);
    };

    match algorithm {
        "egt" => solver.egt_solve(report_every, progress_cb),
        "qre" => solver.solve_with_report_interval(report_every, progress_cb),
        "qre2" => solver.qre_solve(
            qre_lambda,
            qre_damping,
            qre_anneal,
            report_every,
            progress_cb,
        ),
        _ => solver.solve_with_report_interval(report_every, progress_cb),
    }

    let score = score_solver(&solver);
    (solver, score)
}

/// Search for a good single-size postflop strategy.
///
/// `base_config` is the full `SubgameConfig` for the turn spot *without* a
/// bet config (or with only the shared river/flop sizes).  The solver is cloned
/// for each candidate and the turn/river street bet config is replaced.
///
/// `screen_iterations` is the budget used for the coordinate-search screening
/// phase.  `final_iterations` is the budget for the final re-solve with the
/// best size set.
pub fn search_postflop_single_size<F>(
    base_config: &SubgameConfig,
    base_bet_config: &BetConfig,
    candidates: &PostflopSizeCandidates,
    algorithm: &str,
    qre_lambda: f32,
    qre_damping: f32,
    qre_anneal: bool,
    screen_iterations: u32,
    final_iterations: u32,
    early_stop_pct: f32,
    skip_cum_strategy: bool,
    mut progress: F,
) -> (SolveResult, PostflopSizeSet, f32, SubgameSolver)
where
    F: FnMut(SearchProgress),
{
    let max_rounds = 2u32;
    let n_roles = 8usize;

    let default_bet = BetSize::Frac(50, 100);

    // Initialise each role to its first candidate (or a default if empty).
    let mut current = [default_bet; 8];
    for i in 0..n_roles {
        let (street, inner) = PostflopSizeCandidates::street_and_role(i);
        let list = candidates.street_candidates(street).role(inner);
        current[i] = list.first().copied().unwrap_or(default_bet);
    }

    // Convert the flat array back to a PostflopSizeSet.
    let to_set = |arr: [BetSize; 8]| {
        let turn = if candidates.turn.has_any() {
            Some(TurnSizeSet {
                oop_bet: arr[0],
                oop_raise: if candidates.turn.oop_raise.is_empty() { None } else { Some(arr[1]) },
                ip_bet: arr[2],
                ip_raise: if candidates.turn.ip_raise.is_empty() { None } else { Some(arr[3]) },
            })
        } else {
            None
        };
        let river = if candidates.river.has_any() {
            Some(TurnSizeSet {
                oop_bet: arr[4],
                oop_raise: if candidates.river.oop_raise.is_empty() { None } else { Some(arr[5]) },
                ip_bet: arr[6],
                ip_raise: if candidates.river.ip_raise.is_empty() { None } else { Some(arr[7]) },
            })
        } else {
            None
        };
        PostflopSizeSet { turn, river }
    };

    // For the screen phase a slightly looser early-stop target is fine; the
    // goal is only to rank candidates.
    let screen_early_stop = (early_stop_pct * 2.0).max(0.2);

    let mut round = 0u32;
    let mut improved = true;
    while improved && round < max_rounds && candidates.has_any() {
        improved = false;
        round += 1;

        for role_idx in 0..n_roles {
            let role_name = PostflopSizeCandidates::role_name(role_idx);
            let (street, inner_idx) = PostflopSizeCandidates::street_and_role(role_idx);
            let street_candidates = candidates.street_candidates(street);
            let role_candidates = street_candidates.role(inner_idx);
            if role_candidates.is_empty() {
                continue;
            }

            let mut best_score = f32::NEG_INFINITY;
            let mut best_size = current[role_idx];

            for (cand_idx, &size) in role_candidates.iter().enumerate() {
                let mut trial = current;
                trial[role_idx] = size;
                let set = to_set(trial);
                let bet_config = build_postflop_bet_config(base_bet_config, &set);

                let mut last_expl = 0.0f32;
                let mut last_iter = 0u32;
                let (_solver, score) = evaluate_candidate(
                    base_config,
                    bet_config,
                    algorithm,
                    qre_lambda,
                    qre_damping,
                    qre_anneal,
                    screen_iterations,
                    screen_early_stop,
                    skip_cum_strategy,
                    |iter, total, expl| {
                        last_iter = iter;
                        last_expl = expl;
                        progress(SearchProgress {
                            phase: "postflop auto screen",
                            street: street_label(street),
                            round,
                            total_rounds: max_rounds,
                            role: role_name,
                            candidate: cand_idx + 1,
                            total_candidates: role_candidates.len(),
                            iter,
                            total_iter: total,
                            expl_pct: expl,
                        });
                    },
                );

                if score > best_score {
                    best_score = score;
                    best_size = size;
                }

                // Emit one final progress tick for this candidate.
                progress(SearchProgress {
                    phase: "postflop auto screen",
                    street: street_label(street),
                    round,
                    total_rounds: max_rounds,
                    role: role_name,
                    candidate: cand_idx + 1,
                    total_candidates: role_candidates.len(),
                    iter: last_iter.max(1),
                    total_iter: screen_iterations,
                    expl_pct: last_expl,
                });
            }

            if best_size != current[role_idx] {
                current[role_idx] = best_size;
                improved = true;
            }
        }
    }

    let best_set = to_set(current);
    let final_bet_config = build_postflop_bet_config(base_bet_config, &best_set);

    let (final_solver, _) = evaluate_candidate(
        base_config,
        final_bet_config,
        algorithm,
        qre_lambda,
        qre_damping,
        qre_anneal,
        final_iterations,
        early_stop_pct,
        skip_cum_strategy,
        |iter, total, expl| {
            progress(SearchProgress {
                phase: "postflop auto final",
                street: "",
                round: max_rounds,
                total_rounds: max_rounds,
                role: "final",
                candidate: 1,
                total_candidates: 1,
                iter,
                total_iter: total,
                expl_pct: expl,
            });
        },
    );

    let final_score = score_solver(&final_solver);
    let result = SolveResult::from_solver(&final_solver);
    (result, best_set, final_score, final_solver)
}

/// Convenience wrapper for the original turn-only API.
#[allow(dead_code)]
pub fn search_turn_single_size<F>(
    base_config: &SubgameConfig,
    base_bet_config: &BetConfig,
    candidates: &TurnSizeCandidates,
    algorithm: &str,
    qre_lambda: f32,
    qre_damping: f32,
    qre_anneal: bool,
    screen_iterations: u32,
    final_iterations: u32,
    early_stop_pct: f32,
    skip_cum_strategy: bool,
    progress: F,
) -> (SolveResult, TurnSizeSet, f32, SubgameSolver)
where
    F: FnMut(SearchProgress),
{
    let postflop_candidates = PostflopSizeCandidates {
        turn: candidates.clone(),
        river: TurnSizeCandidates {
            oop_bet: Vec::new(),
            oop_raise: Vec::new(),
            ip_bet: Vec::new(),
            ip_raise: Vec::new(),
        },
    };
    let (result, set, score, solver) = search_postflop_single_size(
        base_config,
        base_bet_config,
        &postflop_candidates,
        algorithm,
        qre_lambda,
        qre_damping,
        qre_anneal,
        screen_iterations,
        final_iterations,
        early_stop_pct,
        skip_cum_strategy,
        progress,
    );
    (result, set.turn.unwrap(), score, solver)
}
