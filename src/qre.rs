/// QRE Fixed-Point solver — homotopy continuation
///
/// σ(a|I,c) = exp(λ · CF(a|I,c)) / Σ_b exp(λ · CF(b|I,c))
/// λ = precision: 0 → uniform, ∞ → NE.
/// Homotopy: gradually increase λ from near-zero to target.

use crate::card::{NUM_COMBOS, CARD_COMBOS};
use crate::cfr::{
    SubgameSolver, TreeNode, CfrData, ComboMap, ShowdownCache, SubgameConfig,
    fold_utility, showdown_utility_cached, MAX_ACTIONS,
};
use crate::game::{GameState, Player, OOP, IP};
use rayon::prelude::*;
use std::cell::RefCell;

// ---------------------------------------------------------------------------
// Buffer pool
// ---------------------------------------------------------------------------

thread_local! {
    static QRE_BUF_POOL: RefCell<Vec<Box<[f32; NUM_COMBOS]>>> = const { RefCell::new(Vec::new()) };
    static QRE_ACTION_UTILS_POOL: RefCell<Vec<Vec<[f32; NUM_COMBOS]>>> = const { RefCell::new(Vec::new()) };
}

#[inline]
fn pop_buf() -> Box<[f32; NUM_COMBOS]> {
    QRE_BUF_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| Box::new([0.0f32; NUM_COMBOS]))
    })
}

#[inline]
fn push_buf(buf: Box<[f32; NUM_COMBOS]>) {
    QRE_BUF_POOL.with(|pool| pool.borrow_mut().push(buf));
}

#[inline]
fn pop_action_utils() -> Vec<[f32; NUM_COMBOS]> {
    QRE_ACTION_UTILS_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| vec![[0.0f32; NUM_COMBOS]; MAX_ACTIONS])
    })
}

#[inline]
fn push_action_utils(buf: Vec<[f32; NUM_COMBOS]>) {
    QRE_ACTION_UTILS_POOL.with(|pool| pool.borrow_mut().push(buf));
}

// ---------------------------------------------------------------------------
// Per-node QRE strategy (action-major layout)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct QreStrat {
    strat: Vec<f32>,
    n_actions: usize,
    live_count: usize,
}

impl QreStrat {
    fn new_uniform(n_actions: usize, live_count: usize) -> Self {
        let uniform = 1.0 / n_actions as f32;
        QreStrat {
            strat: vec![uniform; n_actions * live_count],
            n_actions,
            live_count,
        }
    }

    #[inline(always)]
    fn get(&self, action: usize, combo: usize) -> f32 {
        unsafe { *self.strat.get_unchecked(action * self.live_count + combo) }
    }
}

struct QreState {
    strats: [Vec<Option<QreStrat>>; 2],
}

impl QreState {
    fn new(tree: &[TreeNode], cfr: &[CfrData]) -> Self {
        let n_cfr = cfr.len();
        let mut strats = [
            (0..n_cfr).map(|_| None).collect::<Vec<_>>(),
            (0..n_cfr).map(|_| None).collect::<Vec<_>>(),
        ];
        for node in tree {
            if let TreeNode::Decision { player, actions, cfr_idx, .. } = node {
                let p = *player as usize;
                let idx = *cfr_idx as usize;
                if strats[p][idx].is_none() {
                    strats[p][idx] = Some(QreStrat::new_uniform(
                        actions.len(),
                        cfr[idx].live_count,
                    ));
                }
            }
        }
        QreState { strats }
    }
}

// ---------------------------------------------------------------------------
// Gradient buffer — stores per-action CF values
// ---------------------------------------------------------------------------

struct GradBuf {
    data: Vec<Option<Vec<f32>>>,
}

impl GradBuf {
    fn new(n_cfr: usize) -> Self {
        GradBuf {
            data: (0..n_cfr).map(|_| None).collect(),
        }
    }

    fn ensure(&mut self, cfr_idx: usize, n_actions: usize, live_count: usize) {
        if self.data[cfr_idx].is_none() {
            self.data[cfr_idx] = Some(vec![0.0f32; n_actions * live_count]);
        }
    }

    fn get_mut(&mut self, cfr_idx: usize) -> &mut [f32] {
        self.data[cfr_idx].as_mut().unwrap()
    }
}

// Preallocate all slots from the tree to avoid first-iteration allocation spikes.
fn preallocate_grad_buf(grad: &mut GradBuf, tree: &[TreeNode], cfr: &[CfrData]) {
    for node in tree {
        if let TreeNode::Decision { actions, cfr_idx, .. } = node {
            let idx = *cfr_idx as usize;
            grad.ensure(idx, actions.len(), cfr[idx].live_count);
        }
    }
}

// ---------------------------------------------------------------------------
// Target buffer — reusable output for softmax target strategies
// ---------------------------------------------------------------------------

struct TargetBuf {
    data: Vec<Option<Vec<f32>>>,
}

impl TargetBuf {
    fn new(tree: &[TreeNode], cfr: &[CfrData]) -> Self {
        let n_cfr = cfr.len();
        let mut data: Vec<Option<Vec<f32>>> = (0..n_cfr).map(|_| None).collect();
        for node in tree {
            if let TreeNode::Decision { actions, cfr_idx, .. } = node {
                let idx = *cfr_idx as usize;
                if data[idx].is_none() {
                    let len = actions.len() * cfr[idx].live_count;
                    data[idx] = Some(vec![0.0f32; len]);
                }
            }
        }
        TargetBuf { data }
    }

}

// ---------------------------------------------------------------------------
// CF traversal — computes counterfactual values for target_player
// ---------------------------------------------------------------------------

fn cf_traverse(
    tree: &[TreeNode],
    cfr: &[CfrData],
    config: &SubgameConfig,
    sd_cache: &ShowdownCache,
    combo_map: &ComboMap,
    state: &GameState,
    reach: &[[f32; NUM_COMBOS]; 2],
    target_player: Player,
    node_id: u32,
    my_strats: &[Option<QreStrat>],
    opp_strats: &[Option<QreStrat>],
    grad_buf_ptr: *mut GradBuf,
    live_count: usize,
    out: &mut [f32; NUM_COMBOS],
) {
    match &tree[node_id as usize] {
        TreeNode::TerminalFold(folder) => {
            fold_utility(config, state, reach, target_player, *folder, combo_map, out);
        }
        TreeNode::TerminalShowdown => {
            showdown_utility_cached(config, sd_cache, state, reach, target_player, combo_map, out);
        }
        TreeNode::DepthLimitedLeaf { .. } => {
            for v in out[..live_count].iter_mut() { *v = 0.0; }
        }
        TreeNode::Chance { children, iso_perms, n_actual, .. } => {
            if children.is_empty() {
                for v in out[..live_count].iter_mut() { *v = 0.0; }
                return;
            }
            let gbp_usize = grad_buf_ptr as usize;
            let card_utils: Vec<[f32; NUM_COMBOS]> = children
                .par_iter()
                .enumerate()
                .map(|(_i, &(card, child_id))| {
                    let mut next_reach = *reach;
                    for &ci in &CARD_COMBOS[card as usize] {
                        let compact = combo_map.combo_to_live[ci as usize];
                        if compact < 0 { continue; }
                        next_reach[0][compact as usize] = 0.0;
                        next_reach[1][compact as usize] = 0.0;
                    }
                    let next_state = state.deal_street(&[card]);
                    let gbp = gbp_usize as *mut GradBuf;
                    let mut result = [0.0f32; NUM_COMBOS];
                    cf_traverse(
                        tree, cfr, config, sd_cache, combo_map,
                        &next_state, &next_reach, target_player, child_id,
                        my_strats, opp_strats, gbp, live_count,
                        &mut result,
                    );
                    result
                })
                .collect();

            for v in out[..live_count].iter_mut() { *v = 0.0; }
            for (i, cu) in card_utils.iter().enumerate() {
                for c in 0..live_count { out[c] += cu[c]; }
                for perm in &iso_perms[i] {
                    for c in 0..live_count { out[c] += cu[perm[c] as usize]; }
                }
            }
            let inv = 1.0 / *n_actual as f32;
            for c in 0..live_count { out[c] *= inv; }
            out[live_count] = 0.0;
        }
        TreeNode::Decision { player: acting_player, actions, children, cfr_idx } => {
            let acting = *acting_player;
            let cfr_idx_val = *cfr_idx as usize;
            let n_act = actions.len();

            // Zero-reach early out: if either player has no reach at this node,
            // the subtree contributes nothing to the target player's utility.
            let target_p = target_player as usize;
            let opp_p = 1 - target_p;
            let mut target_reach_zero = true;
            let mut opp_reach_zero = true;
            for c in 0..live_count {
                if reach[target_p][c] > 0.0 { target_reach_zero = false; }
                if reach[opp_p][c] > 0.0 { opp_reach_zero = false; }
                if !target_reach_zero && !opp_reach_zero { break; }
            }
            if target_reach_zero || opp_reach_zero {
                for v in out[..live_count].iter_mut() { *v = 0.0; }
                return;
            }

            let use_par = config.par_decision_depth < u32::MAX
                && cfr[cfr_idx_val].decision_depth < config.par_decision_depth
                && n_act > 1;

            if acting == target_player {
                unsafe { (*grad_buf_ptr).ensure(cfr_idx_val, n_act, live_count); }

                let strat = my_strats[cfr_idx_val].as_ref().unwrap();
                for v in out[..live_count].iter_mut() { *v = 0.0; }

                let mut action_utils = pop_action_utils();

                if use_par {
                    let gbp_usize = grad_buf_ptr as usize;
                    action_utils[..n_act].par_iter_mut().enumerate().for_each(|(a, buf)| {
                        let next_state = state.apply(actions[a]);
                        let gbp = gbp_usize as *mut GradBuf;
                        cf_traverse(
                            tree, cfr, config, sd_cache, combo_map,
                            &next_state, reach, target_player, children[a],
                            my_strats, opp_strats, gbp, live_count, buf,
                        );
                    });
                } else {
                    for a in 0..n_act {
                        let next_state = state.apply(actions[a]);
                        cf_traverse(
                            tree, cfr, config, sd_cache, combo_map,
                            &next_state, reach, target_player, children[a],
                            my_strats, opp_strats, grad_buf_ptr, live_count,
                            &mut action_utils[a],
                        );
                    }
                }

                let grad = unsafe { (&mut *grad_buf_ptr).get_mut(cfr_idx_val) };
                for c in 0..live_count {
                    for a in 0..n_act {
                        grad[a * live_count + c] = action_utils[a][c];
                        out[c] += strat.get(a, c) * action_utils[a][c];
                    }
                }

                push_action_utils(action_utils);
            } else {
                let opp_node = opp_strats[cfr_idx_val].as_ref().unwrap();
                for v in out[..live_count].iter_mut() { *v = 0.0; }
                let p = acting as usize;

                // Regret-based pruning for opponent actions: skip actions where no
                // reach flows through (opponent strategy * reach is zero for all combos).
                let mut pruned = [false; MAX_ACTIONS];
                let pruning_active = config.pruning
                    && config.cfr_plus
                    && config.current_iteration >= 50
                    && config.current_iteration % 50 != 0;
                if pruning_active && n_act >= 2 {
                    for a in 0..n_act {
                        let mut flow = 0.0f32;
                        for c in 0..live_count {
                            if reach[p][c] > 0.0 {
                                flow += opp_node.get(a, c) * reach[p][c];
                            }
                        }
                        pruned[a] = flow == 0.0;
                    }
                    let n_pruned = pruned[..n_act].iter().filter(|&&p| p).count();
                    if n_pruned == n_act {
                        pruned[..n_act].fill(false);
                    }
                }

                let mut action_utils = pop_action_utils();

                if use_par {
                    let gbp_usize = grad_buf_ptr as usize;
                    action_utils[..n_act].par_iter_mut().enumerate().for_each(|(a, buf)| {
                        if pruned[a] {
                            for v in buf[..live_count].iter_mut() { *v = 0.0; }
                            return;
                        }
                        let next_state = state.apply(actions[a]);
                        let mut next_reach = *reach;
                        for c in 0..live_count {
                            if next_reach[p][c] > 0.0 {
                                next_reach[p][c] *= opp_node.get(a, c);
                            }
                        }
                        let gbp = gbp_usize as *mut GradBuf;
                        cf_traverse(
                            tree, cfr, config, sd_cache, combo_map,
                            &next_state, &next_reach, target_player, children[a],
                            my_strats, opp_strats, gbp, live_count, buf,
                        );
                    });
                } else {
                    for a in 0..n_act {
                        if pruned[a] { continue; }
                        let next_state = state.apply(actions[a]);
                        let mut next_reach = *reach;
                        for c in 0..live_count {
                            if next_reach[p][c] > 0.0 {
                                next_reach[p][c] *= opp_node.get(a, c);
                            }
                        }
                        cf_traverse(
                            tree, cfr, config, sd_cache, combo_map,
                            &next_state, &next_reach, target_player, children[a],
                            my_strats, opp_strats, grad_buf_ptr, live_count,
                            &mut action_utils[a],
                        );
                    }
                }

                for a in 0..n_act {
                    if pruned[a] { continue; }
                    for c in 0..live_count { out[c] += action_utils[a][c]; }
                }

                push_action_utils(action_utils);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Softmax on CF values: σ(a,c) = exp(λ·CF(a,c)) / Σ exp(λ·CF(b,c))
// ---------------------------------------------------------------------------

fn compute_softmax_target(
    strats: &[Option<QreStrat>],
    grad_buf: &GradBuf,
    lambda: f32,
    targets: &mut TargetBuf,
) {
    let TargetBuf { data } = targets;
    data.par_iter_mut().enumerate().for_each(|(idx, target_opt)| {
        let strat = match &strats[idx] {
            Some(s) => s,
            None => return,
        };
        let grad = match &grad_buf.data[idx] {
            Some(g) => g,
            None => return,
        };
        let target = match target_opt.as_mut() {
            Some(t) => t,
            None => return,
        };
        let n_act = strat.n_actions;
        let lc = strat.live_count;

        for c in 0..lc {
            // log-sum-exp trick for numerical stability
            let mut max_val = f32::NEG_INFINITY;
            for a in 0..n_act {
                let v = lambda * grad[a * lc + c];
                if v > max_val { max_val = v; }
            }

            if !max_val.is_finite() {
                let u = 1.0 / n_act as f32;
                for a in 0..n_act { target[a * lc + c] = u; }
                continue;
            }

            // Compute each exp once, storing it in the target buffer, and skip
            // the wasted exp(1.0) for the max-action (its exponent is 1.0).
            let mut sum_exp = 0.0f32;
            for a in 0..n_act {
                let diff = lambda * grad[a * lc + c] - max_val;
                let e = if diff == 0.0 { 1.0 } else { diff.exp() };
                target[a * lc + c] = e;
                sum_exp += e;
            }

            if sum_exp <= 0.0 || !sum_exp.is_finite() {
                let u = 1.0 / n_act as f32;
                for a in 0..n_act { target[a * lc + c] = u; }
                continue;
            }

            let inv_sum = 1.0 / sum_exp;
            for a in 0..n_act {
                target[a * lc + c] *= inv_sum;
            }
        }
    });
}

/// Apply damped update: strat = (1-α)·strat + α·target
fn apply_damped_update(
    strats: &mut [Option<QreStrat>],
    targets: &TargetBuf,
    alpha: f32,
) {
    strats.par_iter_mut().enumerate().for_each(|(idx, strat_opt)| {
        let strat = match strat_opt.as_mut() {
            Some(s) => s,
            None => return,
        };
        let target = match targets.data[idx].as_ref() {
            Some(t) => t,
            None => return,
        };
        for (s, t) in strat.strat.iter_mut().zip(target.iter()) {
            *s = (1.0 - alpha) * *s + alpha * *t;
        }
    });
}

// ---------------------------------------------------------------------------
// Main QRE solve
// ---------------------------------------------------------------------------

impl SubgameSolver {
    pub fn qre_solve<F: FnMut(u32, &SubgameSolver)>(
        &mut self,
        lambda_target: f32,
        damping: f32,
        anneal: bool,
        report_every: u32,
        mut callback: F,
    ) {
        let reach = self.initial_reach();
        let root_state = self.root_state();

        if self.tree.is_empty() {
            let mut action_seq = Vec::with_capacity(20);
            self.root_id = self.build_tree(&root_state, &mut action_seq, 0);
            self.remap_iso_perms();
            self.precompute_showdowns();
        }

        let live_count = self.combo_map.live_count;
        let n_cfr = self.cfr.len();

        let mut qre = QreState::new(&self.tree, &self.cfr);

        let total = self.config.iterations;
        // Always report the first iteration for UI feedback; then use report_every spacing.
        let mut next_report = 1u32;
        // When report_every > 1, also emit a lightweight progress event every iteration
        // (using the cached exploitability) so the GUI does not appear hung.
        let progress_every = if report_every > 1 { 1 } else { 0 };

        let mut grad_buf_oop = GradBuf::new(n_cfr);
        let mut grad_buf_ip = GradBuf::new(n_cfr);
        preallocate_grad_buf(&mut grad_buf_oop, &self.tree, &self.cfr);
        preallocate_grad_buf(&mut grad_buf_ip, &self.tree, &self.cfr);

        let mut targets_oop = TargetBuf::new(&self.tree, &self.cfr);
        let mut targets_ip = TargetBuf::new(&self.tree, &self.cfr);

        let mut root_util = pop_buf();

        // Late-stage averaging: accumulate strategies from second half of iterations
        let avg_start = total / 2;
        let mut avg_strats: [Vec<Option<Vec<f32>>>; 2] = [
            (0..n_cfr).map(|_| None).collect(),
            (0..n_cfr).map(|_| None).collect(),
        ];
        for p in 0..2 {
            for (idx, s) in qre.strats[p].iter().enumerate() {
                if let Some(strat) = s {
                    avg_strats[p][idx] = Some(vec![0.0f32; strat.strat.len()]);
                }
            }
        }
        let mut avg_count = 0u32;

        let mut arena_allocated = false;
        let mut best_expl = f32::MAX;
        let mut best_cum: Vec<Option<Vec<f32>>> = Vec::new();

        for d in self.cfr.iter_mut() {
            d.cum_strategy = None;
        }
        // Start with a placeholder so lightweight progress callbacks can report 0%
        // before the first expensive exploitability calculation is finished.
        self.last_exploitability_pct = Some(0.0);

        // Homotopy: geometric growth from lambda_start to lambda_target
        let lambda_start = if anneal { (lambda_target * 0.001).max(1e-7) } else { lambda_target };
        let growth = if anneal && lambda_target > lambda_start {
            (lambda_target / lambda_start).powf(1.0 / total as f32)
        } else {
            1.0
        };
        let mut cur_lambda = lambda_start;

        eprintln!("  QRE2: target λ={}, start λ={:.7}, damping={}, growth={:.6}, anneal={}, nodes={}, live={}",
            lambda_target, lambda_start, damping, growth, anneal, self.tree.len(), live_count);

        for iter in 0..total {
            self.iteration = iter + 1;
            self.config.current_iteration = self.iteration;

            if anneal && iter > 0 {
                cur_lambda = (cur_lambda * growth).min(lambda_target);
            }

            // Adaptive damping: reduce alpha as lambda increases to prevent oscillation
            let alpha = if anneal {
                (damping / (1.0 + cur_lambda * 50.0).sqrt()).max(0.05)
            } else {
                damping
            };

            // Sequential (Gauss-Seidel): OOP first, then IP against updated OOP
            {
                let oop_strats = &qre.strats[OOP as usize];
                let ip_strats = &qre.strats[IP as usize];
                cf_traverse(
                    &self.tree, &self.cfr, &self.config, &self.sd_cache,
                    &self.combo_map, &root_state, &reach, OOP, self.root_id,
                    oop_strats, ip_strats, &mut grad_buf_oop as *mut GradBuf, live_count,
                    &mut root_util,
                );
            }
            compute_softmax_target(&qre.strats[OOP as usize], &grad_buf_oop, cur_lambda, &mut targets_oop);
            apply_damped_update(&mut qre.strats[OOP as usize], &targets_oop, alpha);

            {
                let ip_strats = &qre.strats[IP as usize];
                let oop_strats = &qre.strats[OOP as usize];
                cf_traverse(
                    &self.tree, &self.cfr, &self.config, &self.sd_cache,
                    &self.combo_map, &root_state, &reach, IP, self.root_id,
                    ip_strats, oop_strats, &mut grad_buf_ip as *mut GradBuf, live_count,
                    &mut root_util,
                );
            }
            compute_softmax_target(&qre.strats[IP as usize], &grad_buf_ip, cur_lambda, &mut targets_ip);
            apply_damped_update(&mut qre.strats[IP as usize], &targets_ip, alpha);

            // Accumulate for late-stage averaging
            if iter >= avg_start {
                for p in 0..2 {
                    for (idx, s) in qre.strats[p].iter().enumerate() {
                        if let Some(strat) = s {
                            if let Some(ref mut cum) = avg_strats[p][idx] {
                                for (c, v) in cum.iter_mut().zip(strat.strat.iter()) {
                                    *c += *v;
                                }
                            }
                        }
                    }
                }
                avg_count += 1;
            }

            let is_full_report = self.iteration >= next_report || self.iteration == total;
            let is_progress = progress_every > 0 && ((self.iteration - 1) % progress_every == 0);

            if is_full_report {
                if !arena_allocated {
                    let total_elements: usize = self.cfr.iter()
                        .map(|d| d.live_count * d.n_actions)
                        .sum();
                    self.arena = crate::cfr::RegretArena::new(total_elements);
                    let mut offset = 0usize;
                    for data in self.cfr.iter_mut() {
                        data.regret_offset = offset;
                        offset += data.live_count * data.n_actions;
                    }
                    arena_allocated = true;
                }

                // Use late-stage average if available, otherwise current iterate
                if avg_count > 0 {
                    self.qre_copy_averaged_strategy(&avg_strats);
                } else {
                    self.qre_copy_strategy(&qre);
                }
                let expl = self.exploitability_pct();
                self.last_exploitability_pct = Some(expl);
                if expl < best_expl {
                    best_expl = expl;
                    best_cum = self.cfr.iter().map(|d| d.cum_strategy.clone()).collect();
                }
                eprintln!("    λ={:.6} α={:.4}", cur_lambda, alpha);

                // Early-stop support (same logic as solve_with_report_interval)
                if self.config.early_stop_pct > 0.0 {
                    if expl <= self.config.early_stop_pct {
                        self.consecutive_below_threshold += 1;
                    } else {
                        self.consecutive_below_threshold = 0;
                    }
                    if self.consecutive_below_threshold >= self.config.early_stop_patience {
                        break;
                    }
                }

                if report_every > 0 {
                    next_report += report_every;
                } else {
                    while next_report <= self.iteration {
                        next_report = next_report.saturating_mul(2);
                    }
                }
            }

            if is_full_report || is_progress {
                callback(self.iteration, self);
            }
        }

        if !best_cum.is_empty() {
            for (i, d) in self.cfr.iter_mut().enumerate() {
                if i < best_cum.len() {
                    d.cum_strategy = best_cum[i].take();
                }
            }
            eprintln!("  QRE2: restored best iterate (expl={:.4}%)", best_expl);
        }

        push_buf(root_util);
    }

    /// Copy late-stage averaged strategy to cum_strategy
    fn qre_copy_averaged_strategy(&mut self, avg_strats: &[Vec<Option<Vec<f32>>>; 2]) {
        for node in &self.tree {
            if let TreeNode::Decision { player, cfr_idx, actions, .. } = node {
                let p = *player as usize;
                let idx = *cfr_idx as usize;
                let n_act = actions.len();
                let lc = self.cfr[idx].live_count;

                self.cfr[idx].ensure_cum_strategy();
                if let Some(ref mut cum) = self.cfr[idx].cum_strategy {
                    if let Some(ref avg) = avg_strats[p][idx] {
                        for c in 0..lc {
                            let mut sum = 0.0f32;
                            for a in 0..n_act {
                                sum += avg[a * lc + c];
                            }
                            let inv = if sum > 0.0 { 1.0 / sum } else { 0.0 };
                            if inv > 0.0 {
                                for a in 0..n_act {
                                    cum[c * n_act + a] = avg[a * lc + c] * inv;
                                }
                            } else {
                                let u = 1.0 / n_act as f32;
                                for a in 0..n_act {
                                    cum[c * n_act + a] = u;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Copy current QRE iterate to cum_strategy for exploitability measurement
    fn qre_copy_strategy(&mut self, qre: &QreState) {
        for node in &self.tree {
            if let TreeNode::Decision { player, cfr_idx, actions, .. } = node {
                let p = *player as usize;
                let idx = *cfr_idx as usize;
                let n_act = actions.len();
                let lc = self.cfr[idx].live_count;

                self.cfr[idx].ensure_cum_strategy();
                if let Some(ref mut cum) = self.cfr[idx].cum_strategy {
                    let strat = qre.strats[p][idx].as_ref().unwrap();
                    for c in 0..lc {
                        for a in 0..n_act {
                            cum[c * n_act + a] = strat.get(a, c);
                        }
                    }
                }
            }
        }
    }
}
