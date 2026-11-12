//! Cut a recorded take into the strokes of a Nord piano library.
//!
//! A take is one WAV holding every note played. [`crate::wav`] reads it in any PCM or
//! float encoding and saves its regions and loops without touching the audio or any
//! chunk it does not own; [`resample`] brings it to the rate the instrument plays.
//!
//! [`plan`] runs the detectors in turn: [`detect`] finds each hit, its pitch and its
//! key-up, [`hum`] the take's mains hum so levels are measured against the floor
//! without it, [`layers`] assigns hits to notes and chooses each note's layers, and
//! [`cut`] places each hit's attack and release, and [`sustain`] its loop or fade,
//! found on the attack as [`clean`] leaves it. [`plan::Plan::regions`] and
//! [`plan::Plan::loops`] mark them as the build reads them, and the build cleans and
//! shapes each stroke from them.
//!
//! These are tuned heuristics over audio, not declarations of a file format, which is
//! why they sit outside [`crate::formats`].

pub mod clean;
pub mod cut;
pub mod detect;
mod fft;
pub mod filter;
pub mod hum;
pub mod layers;
pub mod plan;
pub mod resample;
pub mod sustain;

/// The most threads [`each`] runs at once. Each tracking block of [`hum::find`] holds
/// about 40 MB while it runs.
pub(crate) const MOST_THREADS: usize = 8;

/// `job` of each of `0..n`, in order. Natively the jobs share up to [`MOST_THREADS`]
/// threads; in the browser, which has none, they run one after another. Each job's
/// result is the same either way.
pub(crate) fn each<T: Send>(n: usize, job: impl Fn(usize) -> T + Sync) -> Vec<T> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(MOST_THREADS)
            .min(n);
        if threads > 1 {
            let job = &job;
            let mut done: Vec<(usize, T)> = std::thread::scope(|scope| {
                let workers: Vec<_> = (0..threads)
                    .map(|first| {
                        scope.spawn(move || {
                            (first..n)
                                .step_by(threads)
                                .map(|i| (i, job(i)))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                workers
                    .into_iter()
                    .flat_map(|worker| worker.join().expect("a take job panicked"))
                    .collect()
            });
            done.sort_by_key(|&(i, _)| i);
            return done.into_iter().map(|(_, result)| result).collect();
        }
    }
    (0..n).map(job).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn each_job_s_result_comes_back_in_order() {
        assert_eq!(
            super::each(37, |i| i * i),
            (0..37).map(|i| i * i).collect::<Vec<_>>()
        );
        assert_eq!(super::each(0, |i| i), Vec::<usize>::new());
    }
}
