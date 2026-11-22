use std::cmp::Ordering;

/// Stable sort that shares one sort instantiation across every caller.
///
/// `slice::sort_by` compiles the whole sort again for each element type and
/// comparator. Use this for sorts over report rows, sessions, models and other
/// short lists. Keep `slice::sort_by` for sorts that grow with loaded entries
/// or discovered files, or that run once per record, where the indirect
/// comparator and the extra position vector cost measurable time.
pub fn stable_by<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    let order = sorted_order(items.len(), &mut |left, right| {
        compare(&items[left], &items[right])
    });
    apply_order(items, order);
}

/// [`stable_by`] keyed by `key`.
pub fn stable_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    stable_by(items, |left, right| key(left).cmp(&key(right)));
}

#[inline(never)]
fn sorted_order(len: usize, compare: &mut dyn FnMut(usize, usize) -> Ordering) -> Vec<usize> {
    let mut order = (0..len).collect::<Vec<_>>();
    order.sort_by(|&left, &right| compare(left, right));
    order
}

fn apply_order<T>(items: &mut [T], mut order: Vec<usize>) {
    for start in 0..order.len() {
        let mut position = start;
        loop {
            let source = order[position];
            order[position] = position;
            if source == start || source == position {
                break;
            }
            items.swap(position, source);
            position = source;
        }
    }
}

#[cfg(test)]
mod tests;
