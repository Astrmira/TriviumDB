use serde_json::Value;
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq)]
pub enum RefValue {
    Missing,
    Null,
    Bool(bool),
    Integer(i128),
    Float(f64),
    String(String),
    List(Vec<RefValue>),
    Path(Vec<u64>),
}

impl RefValue {
    pub fn from_json(value: Option<&Value>) -> Self {
        match value {
            None => Self::Missing,
            Some(Value::Null) => Self::Null,
            Some(Value::Bool(value)) => Self::Bool(*value),
            Some(Value::Number(_)) => match super::model::RefScalar::from_json(value) {
                super::model::RefScalar::Integer(i) => Self::Integer(i),
                super::model::RefScalar::Float(f) => Self::Float(f64::from_bits(f)),
                _ => unreachable!(),
            },
            Some(Value::String(value)) => Self::String(value.clone()),
            Some(Value::Array(values)) => Self::List(
                values
                    .iter()
                    .map(|value| Self::from_json(Some(value)))
                    .collect(),
            ),
            Some(Value::Object(_)) => Self::Missing,
        }
    }

    pub fn total_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Bool(left), Self::Bool(right)) => Some(left.cmp(right)),
            (Self::Integer(left), Self::Integer(right)) => Some(left.cmp(right)),
            (Self::Float(left), Self::Float(right)) if left.is_finite() && right.is_finite() => {
                left.partial_cmp(right)
            }
            (Self::Integer(left), Self::Float(right)) if right.is_finite() => {
                Some(super::model::compare_integer_float(*left, *right))
            }
            (Self::Float(left), Self::Integer(right)) if left.is_finite() => {
                Some(super::model::compare_integer_float(*right, *left).reverse())
            }
            (Self::String(left), Self::String(right)) => Some(left.cmp(right)),
            (Self::Null, Self::Null) | (Self::Missing, Self::Missing) => Some(Ordering::Equal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Literal(RefValue),
    Field(String),
    Add(Box<Expr>, Box<Expr>),
    Subtract(Box<Expr>, Box<Expr>),
    Multiply(Box<Expr>, Box<Expr>),
    Divide(Box<Expr>, Box<Expr>),
    Coalesce(Vec<Expr>),
    IsNull(Box<Expr>),
}

fn number(value: RefValue) -> Option<f64> {
    match value {
        RefValue::Integer(value) => Some(value as f64),
        RefValue::Float(value) if value.is_finite() => Some(value),
        _ => None,
    }
}

pub fn evaluate_expr(payload: &Value, expression: &Expr) -> RefValue {
    match expression {
        Expr::Literal(value) => value.clone(),
        Expr::Field(field) => RefValue::from_json(payload.get(field)),
        Expr::Add(left, right) => {
            integer_arithmetic(payload, left, right, i128::checked_add, |a, b| a + b)
        }
        Expr::Subtract(left, right) => {
            integer_arithmetic(payload, left, right, i128::checked_sub, |a, b| a - b)
        }
        Expr::Multiply(left, right) => {
            integer_arithmetic(payload, left, right, i128::checked_mul, |a, b| a * b)
        }
        Expr::Divide(left, right) => {
            let right_value = number(evaluate_expr(payload, right));
            if right_value == Some(0.0) {
                RefValue::Null
            } else {
                arithmetic(payload, left, right, |a, b| a / b)
            }
        }
        Expr::Coalesce(values) => values
            .iter()
            .map(|value| evaluate_expr(payload, value))
            .find(|value| !matches!(value, RefValue::Null | RefValue::Missing))
            .unwrap_or(RefValue::Null),
        Expr::IsNull(value) => RefValue::Bool(matches!(
            evaluate_expr(payload, value),
            RefValue::Null | RefValue::Missing
        )),
    }
}

// JSON 的整数结果须能由有符号或无符号 64 位整数表示。
fn integer_arithmetic(
    payload: &Value,
    left: &Expr,
    right: &Expr,
    integer: impl Fn(i128, i128) -> Option<i128>,
    float: impl Fn(f64, f64) -> f64,
) -> RefValue {
    match (evaluate_expr(payload, left), evaluate_expr(payload, right)) {
        (RefValue::Integer(left), RefValue::Integer(right)) => integer(left, right)
            .filter(|n| (i128::from(i64::MIN)..=i128::from(u64::MAX)).contains(n))
            .map_or(RefValue::Null, RefValue::Integer),
        _ => arithmetic(payload, left, right, float),
    }
}

fn arithmetic(
    payload: &Value,
    left: &Expr,
    right: &Expr,
    operation: impl Fn(f64, f64) -> f64,
) -> RefValue {
    match (
        number(evaluate_expr(payload, left)),
        number(evaluate_expr(payload, right)),
    ) {
        (Some(left), Some(right)) => {
            let value = operation(left, right);
            if value.is_finite() {
                RefValue::Float(value)
            } else {
                RefValue::Null
            }
        }
        _ => RefValue::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn 值系统区分_missing_null_并支持数值跨类型比较() {
        assert_eq!(RefValue::from_json(None), RefValue::Missing);
        assert_eq!(RefValue::from_json(Some(&Value::Null)), RefValue::Null);
        assert_eq!(
            RefValue::Integer(2).total_cmp(&RefValue::Float(2.0)),
            Some(Ordering::Equal)
        );
        assert!(
            RefValue::Float(f64::NAN)
                .total_cmp(&RefValue::Float(1.0))
                .is_none()
        );
    }

    #[test]
    fn 完整算术_is_null_list_path均有明确语义() {
        let payload = json!({"a": 6, "b": 2, "items": [1, 2]});
        for (expression, expected) in [
            (
                Expr::Add(
                    Box::new(Expr::Field("a".into())),
                    Box::new(Expr::Field("b".into())),
                ),
                RefValue::Integer(8),
            ),
            (
                Expr::Subtract(
                    Box::new(Expr::Field("a".into())),
                    Box::new(Expr::Field("b".into())),
                ),
                RefValue::Integer(4),
            ),
            (
                Expr::Multiply(
                    Box::new(Expr::Field("a".into())),
                    Box::new(Expr::Field("b".into())),
                ),
                RefValue::Integer(12),
            ),
            (
                Expr::IsNull(Box::new(Expr::Field("missing".into()))),
                RefValue::Bool(true),
            ),
        ] {
            assert_eq!(evaluate_expr(&payload, &expression), expected);
        }
        let divide = Expr::Divide(
            Box::new(Expr::Field("a".into())),
            Box::new(Expr::Field("b".into())),
        );
        assert_eq!(evaluate_expr(&payload, &divide), RefValue::Float(3.0));
        let fallback = Expr::Coalesce(vec![
            Expr::Field("missing".into()),
            Expr::Literal(RefValue::Integer(7)),
        ]);
        assert_eq!(evaluate_expr(&payload, &fallback), RefValue::Integer(7));
        assert_eq!(
            RefValue::from_json(payload.get("items")),
            RefValue::List(vec![RefValue::Integer(1), RefValue::Integer(2)])
        );
        assert_eq!(RefValue::Path(vec![1, 2, 3]), RefValue::Path(vec![1, 2, 3]));
    }
}
