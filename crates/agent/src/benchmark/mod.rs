//! Benchmark framework for performance testing

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Benchmark result
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub name: String,
    pub iterations: u64,
    pub mean_duration: Duration,
    pub min_duration: Duration,
    pub max_duration: Duration,
}

/// Simple benchmark harness
pub struct Benchmark;

impl Benchmark {
    pub fn run<F>(name: &str, iterations: u64, mut f: F) -> BenchmarkResult
    where
        F: FnMut(),
    {
        let mut total = Duration::ZERO;
        let mut min = Duration::MAX;
        let mut max = Duration::ZERO;

        for _ in 0..iterations {
            let start = Instant::now();
            f();
            let elapsed = start.elapsed();
            total += elapsed;
            min = min.min(elapsed);
            max = max.max(elapsed);
        }

        BenchmarkResult {
            name: name.to_string(),
            iterations,
            mean_duration: total / iterations as u32,
            min_duration: min,
            max_duration: max,
        }
    }
}

/// Benchmark suite
pub struct BenchmarkSuite {
    benchmarks: Vec<Box<dyn Fn() -> BenchmarkResult>>,
}

impl BenchmarkSuite {
    pub fn new() -> Self {
        Self { benchmarks: Vec::new() }
    }

    pub fn add<F>(&mut self, f: F)
    where
        F: Fn() -> BenchmarkResult + 'static,
    {
        self.benchmarks.push(Box::new(f));
    }

    pub fn run_all(&self) -> Vec<BenchmarkResult> {
        self.benchmarks.iter().map(|f| f()).collect()
    }
}

impl Default for BenchmarkSuite {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark() {
        let result = Benchmark::run("test", 10, || {
            std::thread::sleep(std::time::Duration::from_millis(1));
        });
        assert_eq!(result.iterations, 10);
    }
}
