const MIN_RUNS: usize = 100;
const MAX_RUNS: usize = 10000;
const RSE_TARGET: f64 = 0.01; // 1% RSE

pub trait Statistics {
    fn sample_size(&self) -> f64;
    fn sum(&self) -> f64;
    fn max(&self) -> f64;
    fn min(&self) -> f64;

    fn sample_mean(&self) -> f64 {
        self.sum() / self.sample_size()
    }
    fn sample_variance(&self) -> f64;
    fn sample_standard_deviation(&self) -> f64 {
        self.sample_variance().sqrt()
    }
    fn standard_error_mean(&self) -> f64 {
        let sample_size = self.sample_size();
        let ssd = self.sample_standard_deviation();
        ssd / sample_size.sqrt()
    }
    fn relative_error_mean(&self) -> f64 {
        let sem = self.standard_error_mean();
        let mean = self.sample_mean();
        sem / mean
    }
    fn relative_error_mean_percent(&self) -> f64 {
        self.relative_error_mean() * 100.0
    }
    fn absolute_bound(&self) -> f64 {
        let mean = self.sample_mean();
        (self.max() - mean).abs().max((self.min() - mean).abs())
    }
}

impl Statistics for &Vec<f64> {
    fn sample_size(&self) -> f64 {
        self.len() as f64
    }

    fn sum(&self) -> f64 {
        self.iter().fold(0.0, |acc, v| acc + v)
    }

    fn max(&self) -> f64 {
        self.iter().fold(std::f64::MIN, |acc, v| acc.max(*v))
    }

    fn min(&self) -> f64 {
        self.iter().fold(std::f64::MAX, |acc, v| acc.min(*v))
    }

    fn sample_variance(&self) -> f64 {
        let sample_mean = self.sample_mean();
        let sum = self.iter().fold(0.0, |acc, sample| {
            let err = sample - sample_mean;
            acc + (err * err)
        });
        sum / (self.sample_size() - 1.0)
    }
}

#[derive(Clone, Debug)]
pub struct Stats {
    data: Vec<f64>,
}
impl Stats {
    pub fn new() -> Stats {
        Stats { data: Vec::new() }
    }

    pub fn push(&mut self, v: f64) {
        self.data.push(v);
    }

    #[inline(always)]
    pub fn data(&self) -> &Vec<f64> {
        &self.data
    }

    pub fn is_done(&self) -> bool {
        if self.data().len() >= MIN_RUNS {
            if self.data().len() > MAX_RUNS {
                true
            } else {
                self.relative_error_mean() <= RSE_TARGET
            }
        } else {
            false
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "{:.6}ns,{:.6},{:.6}%",
            self.sample_mean(),
            self.absolute_bound(),
            self.relative_error_mean()
        )
    }
}

impl Statistics for Stats {
    fn sample_size(&self) -> f64 {
        self.data().sample_size()
    }

    fn sum(&self) -> f64 {
        self.data().sum()
    }

    fn max(&self) -> f64 {
        self.data().max()
    }

    fn min(&self) -> f64 {
        self.data().min()
    }

    fn sample_variance(&self) -> f64 {
        self.data().sample_variance()
    }
}
