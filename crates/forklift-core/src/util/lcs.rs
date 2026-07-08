/// For each element of `a`, find the index of the element it maps to in `b` under the
/// longest common subsequence of the two slices (`None` for unmatched elements).
///
/// Classic O(n·m) dynamic program — exact, no heuristics. Used by the merge (diff3) and
/// as the fallback of the display diff for regions without a usable anchor. Callers are
/// responsible for keeping the inputs small enough for the quadratic table.
///
/// # Arguments
/// * `a` - The first slice (the "base" side; the returned vector has one entry per element).
/// * `b` - The second slice.
///
/// # Returns
/// * `Vec<Option<usize>>` - One entry per element of `a`.
pub fn lcs_matches<T: Eq>(a: &[T], b: &[T]) -> Vec<Option<usize>> {
    let n = a.len();
    let m = b.len();

    // table[i][j] = LCS length of a[i..] and b[j..].
    let mut table = vec![vec![0u32; m + 1]; n + 1];

    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }

    let mut matches = vec![None; n];
    let mut i = 0usize;
    let mut j = 0usize;

    while i < n && j < m {
        if a[i] == b[j] {
            matches[i] = Some(j);
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_follow_the_longest_common_subsequence() {
        let a = [1, 2, 3, 4];
        let b = [2, 4, 5];

        assert_eq!(lcs_matches(&a, &b), vec![None, Some(0), None, Some(1)]);
    }

    #[test]
    fn disjoint_slices_have_no_matches() {
        assert_eq!(lcs_matches(&[1, 2], &[3, 4]), vec![None, None]);
    }
}
