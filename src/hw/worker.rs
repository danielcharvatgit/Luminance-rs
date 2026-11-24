//! Background thread that applies hardware brightness/contrast changes.
//!
//! DDC/CI writes take ~50 ms each and logind calls are synchronous, so they
//! must not run on the GTK thread. Requests are coalesced: while one write is
//! in flight, newer values for the same display replace older queued ones, so
//! dragging a slider never builds up a backlog.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::Control;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Feature {
    Brightness,
    Contrast,
}

struct Job {
    key: String,
    feature: Feature,
    control: Control,
    percent: f64,
}

pub struct Worker {
    tx: Sender<Job>,
}

impl Worker {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("luminance-hw".into())
            .spawn(move || run(rx))
            .expect("failed to spawn hardware worker thread");
        Self { tx }
    }

    pub fn set_brightness(&self, key: String, control: Control, percent: f64) {
        let _ = self.tx.send(Job { key, feature: Feature::Brightness, control, percent });
    }

    pub fn set_contrast(&self, key: String, control: Control, percent: f64) {
        let _ = self.tx.send(Job { key, feature: Feature::Contrast, control, percent });
    }
}

fn run(rx: Receiver<Job>) {
    while let Ok(first) = rx.recv() {
        let mut pending: Vec<((String, Feature), Job)> = Vec::new();
        let mut index: HashMap<(String, Feature), usize> = HashMap::new();
        for job in std::iter::once(first).chain(rx.try_iter()) {
            let id = (job.key.clone(), job.feature);
            match index.get(&id) {
                Some(&i) => pending[i].1 = job,
                None => {
                    index.insert(id.clone(), pending.len());
                    pending.push((id, job));
                }
            }
        }
        for (_, job) in pending {
            let result = match job.feature {
                Feature::Brightness => super::set_brightness(&job.control, job.percent),
                Feature::Contrast => super::set_contrast(&job.control, job.percent),
            };
            if let Err(e) = result {
                eprintln!("luminance: {}: {e}", job.key);
            }
        }
    }
}
