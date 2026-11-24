//! 检索增强认知数学算子
//!
//! 提供基于纯 Rust 手写的核心认知搜索数学库：
//! - FISTA (Fast Iterative Shrinkage-Thresholding Algorithm): 稀疏编码残差发现
//! - DPP (Determinantal Point Process): 贪心多样性行列式采样
//! - NMF (Non-negative Matrix Factorization): 语义矩阵分解分析
//!
//! 在 TriviumDB 中完全采用零依赖纯手写，依赖底层 SIMD 进行矩阵加速计算，无缝支持向量管线的深度认知干预。

// ============================================================================
// 辅助函数：SIMD 友好的向量/矩阵运算
// ============================================================================

pub(crate) fn validate_dtype<T: crate::VectorType>() -> crate::Result<()> {
    if crate::vector::is_binary_vector::<T>() {
        return Err(crate::TriviumError::InvalidInput("浮点认知算子要求 f16 或 f32 向量 (Floating-point cognitive operators require f16 or f32 vectors)".into()));
    }
    Ok(())
}

pub(crate) fn checkpoint(control: &crate::query::tql_executor::QueryControl) -> crate::Result<()> {
    #[cfg(feature = "test-hooks")]
    crate::test_hooks::hit(crate::test_hooks::ConcurrencyPoint::CognitiveStep);
    control.check()
}

/// 点积 (利用 Rust 编译器自动向量化)
#[inline]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum()
}

/// L2 范数
#[inline]
pub fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>().sqrt() as f32
}

/// 向量减法: a - b
#[inline]
pub fn vec_sub(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b.iter()).map(|(&x, &y)| x - y).collect()
}

/// 向量加法: a + b
#[inline]
pub fn vec_add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b.iter()).map(|(&x, &y)| x + y).collect()
}

/// 标量乘法: s * v
#[inline]
pub fn vec_scale(v: &[f32], s: f32) -> Vec<f32> {
    v.iter().map(|&x| x * s).collect()
}

/// 矩阵-向量乘法: M (m×m) @ v (m,) -> (m,)
/// M 以行优先 flat array 存储
#[inline]
pub fn mat_vec_mul(m_flat: &[f32], rows: usize, cols: usize, v: &[f32]) -> Vec<f32> {
    debug_assert_eq!(m_flat.len(), rows * cols);
    debug_assert_eq!(v.len(), cols);
    (0..rows)
        .map(|i| {
            let row = &m_flat[i * cols..(i + 1) * cols];
            dot(row, v)
        })
        .collect()
}

/// soft threshold (近端算子 for L1): sign(x) * max(|x| - t, 0)
#[inline]
pub fn soft_threshold(x: &[f32], threshold: f32) -> Vec<f32> {
    x.iter()
        .map(|&v| {
            let abs_v = v.abs();
            if abs_v > threshold {
                v.signum() * (abs_v - threshold)
            } else {
                0.0
            }
        })
        .collect()
}

// ============================================================================
// FISTA 稀疏编码残差 (Beck & Teboulle, 2009)
// ============================================================================

/// FISTA 求解 LASSO: min_α ½‖q - Eᵀα‖² + λ‖α‖₁
///
/// 用于在已经检索到的锚点向量池中，通过稀疏组合逼近 Query。
/// 其未能逼近的部分即为“潜特征残差”。将残差再次投入数据库查询，可能发现弱关联记忆。
///
/// 返回 (alpha, residual_vec, residual_norm)
pub fn fista_solve(
    query: &[f32],         // (d,)
    entities: &[Vec<f32>], // m 个 (d,) 向量
    lambda: f32,
    max_iter: usize,
) -> (Vec<f32>, Vec<f32>, f32) {
    let (weights, residual) = fista_wide(query, entities, lambda, max_iter, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {});
    let norm = residual.iter().map(|v| v * v).sum::<f64>().sqrt();
    (
        weights.into_iter().map(|v| v as f32).collect(),
        residual.into_iter().map(|v| v as f32).collect(),
        norm as f32,
    )
}

/// 余弦召回只需要残差方向。先用宽精度比较绝对阈值，再缩放到可表示的方向。
pub(crate) fn fista_recall_direction(
    query: &[f32],
    entities: &[Vec<f32>],
    lambda: f32,
    iterations: usize,
    threshold: f32,
    control: &crate::query::tql_executor::QueryControl,
) -> crate::Result<Option<Vec<f32>>> {
    let (_, residual) = fista_wide(query, entities, lambda, iterations, || checkpoint(control))?;
    let norm = residual.iter().map(|v| v * v).sum::<f64>().sqrt();
    Ok((norm > f64::from(threshold))
        .then(|| residual.into_iter().map(|v| (v / norm) as f32).collect()))
}

fn fista_wide<E>(
    query: &[f32],
    entities: &[Vec<f32>],
    lambda: f32,
    max_iter: usize,
    check: impl Fn() -> Result<(), E>,
) -> Result<(Vec<f64>, Vec<f64>), E> {
    check()?;
    let m = entities.len();
    let mut residual_wide: Vec<_> = query.iter().copied().map(f64::from).collect();
    if m == 0 {
        return Ok((Vec::new(), residual_wide));
    }

    // 先扩宽分量再相乘，使极大和极小的有限输入共用同一个求解过程。
    let wide_dot = |left: &[f32], right: &[f32]| -> f64 {
        left.iter()
            .zip(right)
            .map(|(&x, &y)| f64::from(x) * f64::from(y))
            .sum()
    };
    let mut gram = vec![0.0; m * m];
    for i in 0..m {
        check()?;
        for j in i..m {
            let value = wide_dot(&entities[i], &entities[j]);
            gram[i * m + j] = value;
            gram[j * m + i] = value;
        }
    }
    let lip = gram
        .chunks_exact(m)
        .map(|row| row.iter().map(|value| value.abs()).sum::<f64>())
        .fold(0.0, f64::max);
    if lip == 0.0 {
        return Ok((vec![0.0; m], residual_wide));
    }
    let target: Vec<_> = entities
        .iter()
        .map(|entity| wide_dot(entity, query))
        .collect();
    let threshold = f64::from(lambda) / lip;
    let mut alpha = vec![0.0; m];
    let mut next = vec![0.0; m];
    let mut y = vec![0.0; m];
    let mut t = 1.0_f64;
    for _ in 0..max_iter {
        check()?;
        for (i, row) in gram.chunks_exact(m).enumerate() {
            let projected: f64 = row.iter().zip(&y).map(|(g, v)| g * v).sum();
            let step = y[i] + (target[i] - projected) / lip;
            next[i] = (step.abs() - threshold).max(0.0).copysign(step);
        }
        let next_t = (1.0 + (1.0 + 4.0 * t * t).sqrt()) / 2.0;
        for i in 0..m {
            y[i] = next[i] + (t - 1.0) / next_t * (next[i] - alpha[i]);
        }
        std::mem::swap(&mut alpha, &mut next);
        t = next_t;
    }

    for (&weight, entity) in alpha.iter().zip(entities) {
        check()?;
        for (value, &component) in residual_wide.iter_mut().zip(entity) {
            *value -= weight * f64::from(component);
        }
    }
    Ok((alpha, residual_wide))
}

// ============================================================================
// DPP 贪心多样性采样 (Kulesza & Taskar, 2012)
// ============================================================================

/// 贪心 DPP: 增量 Cholesky 分解行列式采样算法
///
/// 目的是在 `candidates` 中选取不重复的 `k` 个样本。兼顾个体的得分 `scores`，
/// 以及它们在向量空间彼此之间的多样性（互相距离越远越好）。
pub fn dpp_greedy(
    vecs: &[Vec<f32>], // (N, d)
    scores: &[f32],    // (N,)
    k: usize,
    quality_weight: f32,
) -> Vec<usize> {
    let scores = scores
        .iter()
        .map(|&score| f64::from(score))
        .collect::<Vec<_>>();
    dpp_greedy_wide(vecs, &scores, k, quality_weight, || {
        Ok::<(), std::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {})
}

pub(crate) fn dpp_greedy_wide<E>(
    vecs: &[Vec<f32>],
    scores: &[f64],
    k: usize,
    quality_weight: f32,
    check: impl Fn() -> Result<(), E>,
) -> Result<Vec<usize>, E> {
    check()?;
    let n = scores.len();
    let k = k.min(n);
    if k == 0 {
        return Ok(Vec::new());
    }

    // L2 归一化，用于计算余弦相似度核
    let normed: Vec<Vec<f32>> = vecs
        .iter()
        .map(|v| {
            check()?;
            let norm = v.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>().sqrt();
            // 空切片表示零方向，其自内积和与任何方向的内积均为零。
            if norm == 0.0 {
                return Ok(Vec::new());
            }
            let inv = norm.recip();
            Ok(v.iter().map(|&x| (f64::from(x) * inv) as f32).collect())
        })
        .collect::<Result<_, E>>()?;

    // 在对数域统一缩放核与正则项，保留行列式排序并限制所有质量因子的上界。
    let log_q: Vec<f64> = scores
        .iter()
        .map(|&s| s.max(1e-10).ln() * f64::from(quality_weight))
        .collect();
    let log_scale = log_q.iter().copied().fold((1e-8f64).ln() / 2.0, f64::max);
    let q: Vec<f64> = log_q
        .iter()
        .map(|&value| (value - log_scale).exp())
        .collect();
    let regularization = ((1e-8f64).ln() - 2.0 * log_scale).exp();
    let stopping_variance = ((1e-10f64).ln() - 2.0 * log_scale).exp();

    // d[i] 保存缩放后的条件方差，正则项与质量核使用同一比例。
    let mut diag: Vec<f64> = q
        .iter()
        .zip(&normed)
        .map(|(&qi, vector)| {
            // 非零方向归一化后的自内积精确为一，避免分量舍入改变同质量排序。
            if vector.is_empty() {
                regularization
            } else {
                qi * qi + regularization
            }
        })
        .collect();

    // Cholesky 增量列: O(nk^2)
    let mut c = vec![vec![0.0f64; n]; k];

    let mut selected = Vec::with_capacity(k);

    for j in 0..k {
        check()?;
        // 找条件方差 d[i] 最大的（即边际极大化行列式的）
        let mut best = None;
        let mut best_val = f64::NEG_INFINITY;
        for i in 0..n {
            if diag[i] > best_val && !selected.contains(&i) {
                best_val = diag[i];
                best = Some(i);
            }
        }
        let Some(best) = best else { break };
        selected.push(best);

        if j == k - 1 {
            break;
        }

        // 已经无法增加行列式总值（池子里剩下的全都是高度相似的冗余内容）
        if diag[best] <= stopping_variance {
            break;
        }

        // 惰性求值构建 L 矩阵
        let l_best_row: Vec<f64> = (0..n)
            .map(|i| {
                check()?;
                let similarity = normed[best]
                    .iter()
                    .zip(&normed[i])
                    .map(|(&a, &b)| f64::from(a) * f64::from(b))
                    .sum::<f64>();
                Ok(q[best] * similarity * q[i])
            })
            .collect::<Result<_, E>>()?;

        // 增量消元: c[j, :] = L[best, :] - Σ_{i<j} c[i, best] * c[i, :]
        let mut cj = l_best_row;
        for i in 0..j {
            check()?;
            let c_i_best = c[i][best];
            for idx in 0..n {
                cj[idx] -= c_i_best * c[i][idx];
            }
        }

        let inv_sqrt = 1.0 / diag[best].sqrt();
        for idx in 0..n {
            cj[idx] *= inv_sqrt;
        }

        // 更新剩余候选的条件方差: d -= c[j]^2
        for i in 0..n {
            diag[i] -= cj[i] * cj[i];
            if diag[i] < 0.0 {
                diag[i] = 0.0;
            }
        }

        c[j] = cj;
    }

    Ok(selected)
}

// ============================================================================
// NMF 非负矩阵分解 (Lee & Seung, 1999)
// ============================================================================

/// NMF 乘法更新算法: V ≈ W × H
///
/// 将大量实体嵌入矩阵分解，提炼出隐特征。
/// * V: (m, d), W: (m, k), H: (k, d)
///
/// 返回: (W, H) 行优先平坦组合
pub fn nmf_multiplicative_update(
    v_flat: &[f32],
    m: usize,
    d: usize,
    k: usize,
    max_iter: usize,
    tol: f32,
) -> crate::Result<(Vec<f32>, Vec<f32>)> {
    let control = crate::request::control().unwrap_or_default();
    nmf_checked(v_flat, m, d, k, max_iter, tol, &control)
}

pub(crate) fn nmf_checked(
    v_flat: &[f32],
    m: usize,
    d: usize,
    k: usize,
    max_iter: usize,
    tol: f32,
    control: &crate::query::tql_executor::QueryControl,
) -> crate::Result<(Vec<f32>, Vec<f32>)> {
    checkpoint(control)?;
    if m == 0
        || d == 0
        || k == 0
        || m.checked_mul(d) != Some(v_flat.len())
        || [(m, d), (m, k), (k, d), (k, k)].iter().any(|&(a, b)| {
            a.checked_mul(b)
                .is_none_or(|n| n > isize::MAX as usize / size_of::<f64>())
        })
    {
        return Err(crate::TriviumError::InvalidInput(
            "NMF 矩阵形状与元素数量无效 (Invalid NMF matrix shape or element count)".into(),
        ));
    }
    let eps = 1e-10f64;

    // 绝对值截断保证非负
    let mut v_abs = vec![0.0f64; m * d];
    for (i, &val) in v_flat.iter().enumerate() {
        v_abs[i] = f64::from(val).abs();
    }

    let v_mean = v_abs.iter().sum::<f64>() / (m * d) as f64;
    let avg = (v_mean / k as f64).sqrt().max(0.01);

    // 极简实现的伪随机数，剥离 rand 依赖
    let mut seed: u64 = 42;
    let mut next_rand = || -> f64 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let val = ((seed >> 33) as f64) / (u32::MAX as f64);
        (avg + avg * 0.5 * (val - 0.5)).abs() + eps
    };

    // f32 事实输入使用 f64 工作区，容纳乘法更新与残差平方的中间量。
    let mut w: Vec<f64> = (0..m * k).map(|_| next_rand()).collect();
    let mut h: Vec<f64> = (0..k * d).map(|_| next_rand()).collect();

    for iter in 0..max_iter {
        // --- 1. 更新隐特征 H ---
        let mut wtv = vec![0.0f64; k * d];
        for ki in 0..k {
            checkpoint(control)?;
            for di in 0..d {
                let mut sum = 0.0f64;
                for mi in 0..m {
                    sum += w[mi * k + ki] * v_abs[mi * d + di];
                }
                wtv[ki * d + di] = sum;
            }
        }

        let mut wtw = vec![0.0f64; k * k];
        for i in 0..k {
            checkpoint(control)?;
            for j in i..k {
                let mut sum = 0.0f64;
                for mi in 0..m {
                    sum += w[mi * k + i] * w[mi * k + j];
                }
                wtw[i * k + j] = sum;
                wtw[j * k + i] = sum;
            }
        }

        let mut wtwh = vec![0.0f64; k * d];
        for i in 0..k {
            checkpoint(control)?;
            for di in 0..d {
                let mut sum = 0.0f64;
                for j in 0..k {
                    sum += wtw[i * k + j] * h[j * d + di];
                }
                wtwh[i * d + di] = sum;
            }
        }

        // H 规则迭代
        for i in 0..k * d {
            h[i] *= wtv[i] / (wtwh[i] + eps);
        }

        // --- 2. 更新权重基 W ---
        let mut vht = vec![0.0f64; m * k];
        for mi in 0..m {
            checkpoint(control)?;
            for ki in 0..k {
                checkpoint(control)?;
                let mut sum = 0.0f64;
                for di in 0..d {
                    sum += v_abs[mi * d + di] * h[ki * d + di];
                }
                vht[mi * k + ki] = sum;
            }
        }

        let mut hht = vec![0.0f64; k * k];
        for i in 0..k {
            checkpoint(control)?;
            for j in i..k {
                let mut sum = 0.0f64;
                for di in 0..d {
                    sum += h[i * d + di] * h[j * d + di];
                }
                hht[i * k + j] = sum;
                hht[j * k + i] = sum;
            }
        }

        let mut whht = vec![0.0f64; m * k];
        for mi in 0..m {
            checkpoint(control)?;
            for ki in 0..k {
                checkpoint(control)?;
                let mut sum = 0.0f64;
                for j in 0..k {
                    sum += w[mi * k + j] * hht[j * k + ki];
                }
                whht[mi * k + ki] = sum;
            }
        }

        // W 规则迭代
        for i in 0..m * k {
            w[i] *= vht[i] / (whht[i] + eps);
        }

        // 收敛残差
        if iter > 0 && iter % 10 == 0 {
            let mut res_sq = 0.0f64;
            let mut v_sq = 0.0f64;
            for mi in 0..m {
                checkpoint(control)?;
                for di in 0..d {
                    let mut wh = 0.0f64;
                    for ki in 0..k {
                        wh += w[mi * k + ki] * h[ki * d + di];
                    }
                    let diff = v_abs[mi * d + di] - wh;
                    res_sq += diff * diff;
                    v_sq += v_abs[mi * d + di] * v_abs[mi * d + di];
                }
            }
            if v_sq > 0.0 && (res_sq / v_sq).sqrt() < f64::from(tol) {
                break;
            }
        }
    }

    Ok((
        w.into_iter().map(|value| value as f32).collect(),
        h.into_iter().map(|value| value as f32).collect(),
    ))
}

/// NMF 查询深度分析：评估原意图中的不均衡性与新领域
///
/// 返回: (semantic_depth, topic_coverage, novelty, top_topics_distribution)
pub fn nmf_analyze_query(
    query: &[f32],  // (d,)
    h_flat: &[f32], // (k, d)
    k: usize,
    d: usize,
) -> (f32, usize, f32, Vec<f32>) {
    let q_abs: Vec<f64> = query.iter().map(|&x| f64::from(x).abs()).collect();
    let mut raw_scores = vec![0.0f64; k];
    for (ki, score) in raw_scores.iter_mut().enumerate() {
        *score = q_abs
            .iter()
            .zip(&h_flat[ki * d..(ki + 1) * d])
            .map(|(q, &h)| q * f64::from(h))
            .sum();
    }
    let max_s = raw_scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for s in &mut raw_scores {
        *s = (*s - max_s).exp();
    }
    let sum_exp: f64 = raw_scores.iter().sum();
    let q_topics: Vec<f32> = raw_scores.iter().map(|s| (s / sum_exp) as f32).collect();

    // 1. 语义聚焦深度 = 1 - 归一化信息熵 (完全均匀发布意味着毫无深度)
    let entropy: f32 = q_topics
        .iter()
        .map(|&p| if p > 1e-10 { -p * p.ln() } else { 0.0 })
        .sum();
    let max_entropy = if k > 1 { (k as f32).ln() } else { 1.0 };
    let semantic_depth = 1.0 - entropy / max_entropy;

    // 2. 覆盖话题广度
    let threshold = 0.5 / k as f32;
    let topic_coverage = q_topics.iter().filter(|&&p| p > threshold).count();

    // 3. 意图新颖脱轨程度: 当 H 矩阵重构不出 Query 的真实全貌，即代表有未被索引的“新颖逻辑”
    let mut q_recon = vec![0.0f64; d];
    for ki in 0..k {
        let w = q_topics[ki];
        if w > 1e-10 {
            for di in 0..d {
                q_recon[di] += f64::from(w) * f64::from(h_flat[ki * d + di]);
            }
        }
    }
    let diff = q_abs
        .iter()
        .zip(q_recon)
        .map(|(q, r)| (q - r).powi(2))
        .sum::<f64>()
        .sqrt();
    let q_norm = q_abs.iter().map(|v| v * v).sum::<f64>().sqrt();
    let novelty = if q_norm > 0.0 {
        (diff / q_norm).min(1.0) as f32
    } else {
        f32::from(diff > 0.0)
    };

    (semantic_depth, topic_coverage, novelty, q_topics)
}
