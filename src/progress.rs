const FILLED: &str = "########################################";
const EMPTY: &str = "........................................";

pub(crate) struct Progress {
    name: &'static str,
    total: u64,
    done: u64,
    last: i64,
}
impl Progress {
    pub(crate) fn new(name: &'static str, total: u64) -> Self {
        let total = total.max(1);
        eprint!("{}: 0.0%", name);
        Progress {
            name,
            total,
            done: 0,
            last: -1,
        }
    }
    pub(crate) fn add(&mut self, n: u64) {
        self.done += n;
        let pct = (self.done.saturating_mul(1000) / self.total) as i64;
        if pct != self.last {
            self.last = pct;
            let filled = (pct as usize * 40 / 1000).min(40);
            eprint!(
                "\r{}: [{}{}] {:>5.1}% ({}/{})",
                self.name,
                &FILLED[..filled],
                &EMPTY[..40 - filled],
                pct as f64 / 10.0,
                self.done,
                self.total
            );
        }
    }
    pub(crate) fn finish(&self) {
        eprintln!("\r{}: 100.0% ({}/{})", self.name, self.total, self.total);
    }
}
