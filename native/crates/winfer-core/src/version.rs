//! Version order for updates (the desktop's and winferd's).

fn parts(v: &str) -> (Vec<u64>, Option<Vec<u64>>) {
    let v = v.trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (v, None),
    };
    let nums = |s: &str| s.split('.').map(|p| p.parse().unwrap_or(0)).collect::<Vec<u64>>();
    (nums(core), pre.map(nums))
}

fn cmp_nums(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    for i in 0..a.len().max(b.len()) {
        let o = a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0));
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    std::cmp::Ordering::Equal
}

/// Orders "0.1.9" before "0.1.10"; a leading "v" is ignored. A numeric pre-release sorts before its release.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (ca, pa) = parts(a);
    let (cb, pb) = parts(b);
    let o = cmp_nums(&ca, &cb);
    if o != std::cmp::Ordering::Equal {
        return o;
    }
    match (pa, pb) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => cmp_nums(&x, &y),
    }
}

#[cfg(test)]
mod tests {
    use super::compare_versions;
    use std::cmp::Ordering;

    #[test]
    fn orders_versions() {
        assert_eq!(compare_versions("0.1.9", "0.1.10"), Ordering::Less);
        assert_eq!(compare_versions("0.1.102-7", "0.1.102"), Ordering::Less);
        assert_eq!(compare_versions("v0.1.103", "0.1.102-9"), Ordering::Greater);
    }
}
