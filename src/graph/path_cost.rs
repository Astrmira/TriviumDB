//! 非负边权的精确累加与比较。只在 f64 加法发生舍入时展开为二进制整数。

use std::cmp::Ordering;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub(crate) enum PathCost {
    Float(f64),
    // 每个字按 2^-1074 为单位存储，对齐到 64 位；跳过值为零的字。
    Words(BTreeMap<usize, u64>),
}

impl PathCost {
    pub(super) fn plus(&self, weight: f64) -> Self {
        if let Self::Float(value) = *self {
            let (hi, lo) = (value.max(weight), value.min(weight));
            let sum = hi + lo;
            if sum.is_finite() && sum - hi == lo {
                return Self::Float(sum);
            }
        }
        let mut words = self.words();
        Self::add_weight(&mut words, weight);
        Self::Words(words)
    }

    fn words(&self) -> BTreeMap<usize, u64> {
        match self {
            Self::Words(words) => words.clone(),
            Self::Float(value) => {
                let mut words = BTreeMap::new();
                Self::add_weight(&mut words, *value);
                words
            }
        }
    }

    fn add_weight(words: &mut BTreeMap<usize, u64>, weight: f64) {
        if weight == 0.0 {
            return;
        }
        let bits = weight.to_bits();
        let exp = (bits >> 52) as usize;
        let significand = (bits & ((1 << 52) - 1)) | (u64::from(exp != 0) << 52);
        let shift = exp.saturating_sub(1);
        let mut carry = u128::from(significand) << (shift % 64);
        let mut index = shift / 64;
        while carry != 0 {
            carry += u128::from(words.get(&index).copied().unwrap_or(0));
            let word = carry as u64;
            if word == 0 {
                words.remove(&index);
            } else {
                words.insert(index, word);
            }
            carry >>= 64;
            index += 1;
        }
    }

    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Words(a), Self::Words(b)) => a.iter().rev().cmp(b.iter().rev()),
            (Self::Words(a), b) => a.iter().rev().cmp(b.words().iter().rev()),
            (a, Self::Words(b)) => a.words().iter().rev().cmp(b.iter().rev()),
        }
    }

    pub(crate) fn finite_value(&self) -> crate::Result<f64> {
        let cost = self.value();
        if cost.is_finite() {
            Ok(cost)
        } else {
            Err(crate::TriviumError::QueryExecution(
                "路径成本超出 f64 范围 (Path cost exceeds f64 range)".into(),
            ))
        }
    }

    /// 只在返回结果时按最近偶数舍入。正常数据库的有限 f32 边权总和在 f64 范围内。
    pub(super) fn value(&self) -> f64 {
        let words = match self {
            Self::Float(value) => return *value,
            Self::Words(words) => words,
        };
        let Some((&index, &top)) = words.last_key_value() else {
            return 0.0;
        };
        let high_bit = 64 * index + 63 - top.leading_zeros() as usize;
        if high_bit <= 52 {
            return f64::from_bits(top);
        }
        if high_bit > 2097 {
            return f64::INFINITY;
        }
        let shift = high_bit - 52;
        let at = |i| words.get(&i).copied().unwrap_or(0);
        let pair = u128::from(at(shift / 64)) | (u128::from(at(shift / 64 + 1)) << 64);
        let significand = (pair >> (shift % 64)) as u64;
        let guard = shift - 1;
        let guard_bit = 1u64 << (guard % 64);
        let low = at(guard / 64);
        let sticky = low & (guard_bit - 1) != 0 || words.range(..guard / 64).next().is_some();
        let round = low & guard_bit != 0 && (sticky || significand & 1 != 0);
        f64::from_bits(((shift as u64) << 52) + significand + u64::from(round))
    }
}

#[cfg(test)]
mod tests {
    use super::PathCost;
    use std::cmp::Ordering;

    #[test]
    fn 成本按精确值排序并仅在输出时舍入到最近偶数() {
        let one = PathCost::Float(1.);
        let half_ulp = f64::EPSILON / 2.;
        let tie = one.plus(half_ulp);
        let above = tie.plus(f64::from_bits(1));
        assert_eq!(one.compare(&tie), Ordering::Less);
        assert_eq!(tie.compare(&above), Ordering::Less);
        assert_eq!(tie.value(), 1.);
        assert_eq!(above.value(), 1f64.next_up());
        assert_eq!(
            PathCost::Float(1f64.next_up()).plus(half_ulp).value(),
            1. + 2. * f64::EPSILON
        );
        assert_eq!(
            PathCost::Float(f64::MAX)
                .plus(f64::MAX)
                .finite_value()
                .unwrap_err()
                .code(),
            "TDB_QUERY_EXECUTION"
        );
    }
}
