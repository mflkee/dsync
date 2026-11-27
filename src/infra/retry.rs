//! Обобщённое retry с backoff и политикой «повторять ли ошибку».

use std::future::Future;
use std::time::Duration;

/// Результат retry-обёртки: значение/ошибка и число сделанных попыток.
#[derive(Debug)]
pub struct Outcome<T, E> {
    pub value: Result<T, E>,
    pub attempts: u32,
}

/// Выполнить `op` до `max_attempts` раз, повторяя только «транзиентные»
/// ошибки (`is_transient`). Пауза между попытками стартует с `base` и
/// удваивается до `cap`; после последней попытки паузы нет.
pub async fn retry<F, Fut, T, E, P>(
    max_attempts: u32,
    base: Duration,
    cap: Duration,
    mut op: F,
    is_transient: P,
) -> Outcome<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, E>>,
    P: Fn(&E) -> bool,
{
    let max = max_attempts.max(1);
    let mut delay = base;
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        match op(attempts).await {
            Ok(v) => {
                return Outcome {
                    value: Ok(v),
                    attempts,
                }
            }
            Err(e) => {
                if attempts >= max || !is_transient(&e) {
                    return Outcome {
                        value: Err(e),
                        attempts,
                    };
                }
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                delay = delay.saturating_mul(2).min(cap);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn succeeds_first_time() {
        let out = retry(
            3,
            Duration::from_millis(10),
            Duration::from_secs(1),
            |_| async { Ok::<_, String>(7) },
            |_: &String| true,
        )
        .await;
        assert_eq!(out.attempts, 1);
        assert_eq!(out.value.unwrap(), 7);
    }

    #[tokio::test(start_paused = true)]
    async fn retries_transient_until_success() {
        let mut calls = 0u32;
        let out = retry(
            4,
            Duration::from_millis(10),
            Duration::from_secs(1),
            |_| {
                calls += 1;
                let n = calls;
                async move {
                    if n < 3 {
                        Err("flaky")
                    } else {
                        Ok(n)
                    }
                }
            },
            |_: &&str| true,
        )
        .await;
        assert_eq!(out.attempts, 3);
        assert_eq!(out.value.unwrap(), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn stops_on_non_transient() {
        let mut calls = 0u32;
        let out = retry(
            5,
            Duration::from_millis(10),
            Duration::from_secs(1),
            |_| {
                calls += 1;
                async { Err::<(), _>("fatal") }
            },
            |e: &&str| *e != "fatal",
        )
        .await;
        assert_eq!(out.attempts, 1);
        assert_eq!(out.value.unwrap_err(), "fatal");
        assert_eq!(calls, 1);
    }
}
