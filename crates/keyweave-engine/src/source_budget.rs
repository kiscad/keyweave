//! Static source-identity ceiling for one physical root of the stage pipeline.
use crate::*;
impl ConfigSnapshot {
    /// Reserve every possible descendant once, including retained snapshot scopes.
    /// Includes Simple main/after-up; other Simple branches remain rejected.
    /// The result is a conservative lease, not an actual owner count or SourceToken allocator.
    pub fn timed_stage_source_bounds(&self) -> BTreeMap<KeyId, usize> {
        let weight = |actions: &[Action]| {
            actions
                .iter()
                .map(|action| 1usize.saturating_add(action.modifiers.len()))
                .fold(0usize, usize::saturating_add)
        };
        let mut complex = BTreeMap::<KeyId, usize>::new();
        for mapping in &self.complex {
            let extras = &mapping.extras;
            let mut output = weight(&mapping.to)
                .saturating_add(weight(&extras.if_held_down))
                .saturating_add(weight(&extras.after_key_up));
            for actions in [&extras.if_alone] {
                if !actions.is_empty() {
                    output = output.saturating_add(weight(actions)).saturating_add(8);
                }
            }
            if let Some(delayed) = &extras.delayed {
                for actions in [&delayed.if_invoked, &delayed.if_canceled] {
                    if !actions.is_empty() {
                        output = output.saturating_add(weight(actions)).saturating_add(8);
                    }
                }
            }
            // One Complex identity, plus selected output or pass-through.
            let cost = 1usize.saturating_add(output.max(1));
            complex
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(cost))
                .or_insert(cost);
        }
        let mut bounds: BTreeMap<_, _> = complex
            .iter()
            .map(|(&key, &cost)| (key, cost.saturating_add(1)))
            .collect();
        for mapping in &self.simple {
            let descendants = mapping
                .to
                .iter()
                .chain(&mapping.extras.after_key_up)
                .map(|action| {
                    let main = match action.target {
                        ActionTarget::Key(key) => complex.get(&key).copied().unwrap_or(2),
                        ActionTarget::SetVariable(_) => 2,
                    };
                    action
                        .modifiers
                        .iter()
                        .map(|key| complex.get(key).copied().unwrap_or(2))
                        .fold(main, usize::saturating_add)
                })
                .fold(0usize, usize::saturating_add);
            let cost = 1usize.saturating_add(descendants);
            let fallback = complex
                .get(&mapping.from)
                .copied()
                .unwrap_or(2)
                .saturating_add(1);
            bounds
                .entry(mapping.from)
                .and_modify(|bound| *bound = (*bound).max(cost))
                .or_insert(cost.max(fallback));
        }
        bounds
    }
}
