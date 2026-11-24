use std::collections::HashSet;
use std::ops::RangeInclusive;

/// 对同一工作点的全部查询计分；样本为无过滤且至少含 K 条记录的语料。
pub fn at_k<T>(
    answers: &[Vec<T>],
    truth: &[Vec<u64>],
    k: usize,
    ids: RangeInclusive<u64>,
    hit: impl Fn(&T) -> (u64, f32),
) -> f64 {
    assert!(k > 0 && !truth.is_empty(), "召回计分需要查询和非零 K");
    assert_eq!(answers.len(), truth.len(), "查询数与真值不符");
    let mut matched = 0usize;
    for (query, (rows, expected)) in answers.iter().zip(truth).enumerate() {
        assert_eq!(expected.len(), k, "查询 {query} 真值条数不符");
        let expected: HashSet<_> = expected.iter().copied().collect();
        assert_eq!(expected.len(), k, "查询 {query} 真值 ID 重复");
        assert!(
            expected.iter().all(|id| ids.contains(id)),
            "查询 {query} 真值 ID 越界"
        );
        assert_eq!(rows.len(), k, "查询 {query} 返回条数不符");
        let mut seen = HashSet::with_capacity(k);
        for row in rows {
            let (id, score) = hit(row);
            assert!(ids.contains(&id), "查询 {query} 返回未知 ID {id}");
            assert!(score.is_finite(), "查询 {query} 的 ID {id} 分数非有限");
            assert!(seen.insert(id), "查询 {query} 重复返回 ID {id}");
            matched += usize::from(expected.contains(&id));
        }
    }
    matched as f64 / truth.len() as f64 / k as f64
}
