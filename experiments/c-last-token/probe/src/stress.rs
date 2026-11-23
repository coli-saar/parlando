//! Exercise multiple independent Wasmtime stores with a bounded memory budget.
use parlando::component::{ComponentValue, LoadedGame};
use parlando::PlayerRole;
use std::ffi::c_void;
use std::io;
use std::sync::Barrier;
use std::thread;
use std::time::Instant;

const MIB: f64 = 1_048_576.0;
const MAX_FOOTPRINT: u64 = 3_328 * 1_048_576;
const WORKERS: usize = 4;
const TURNS: usize = 12;

/// Layout of Darwin's rusage_info_v0 from sys/resource.h.
#[repr(C)]
#[derive(Default)]
struct Rusage {
    uuid: [u8; 16],
    user_time: u64,
    system_time: u64,
    pkg_idle_wkups: u64,
    interrupt_wkups: u64,
    pageins: u64,
    wired_size: u64,
    resident_size: u64,
    phys_footprint: u64,
    proc_start_abstime: u64,
    proc_exit_abstime: u64,
}

#[link(name = "proc")]
extern "C" {
    fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut c_void) -> i32;
}

/// Read current resident bytes and macOS's physical footprint for this process.
fn memory() -> io::Result<Rusage> {
    let mut info = Rusage::default();
    // Flavor zero writes an initialized rusage_info_v0 into the provided buffer.
    let result = unsafe {
        proc_pid_rusage(
            std::process::id() as i32,
            0,
            (&mut info as *mut Rusage).cast(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

/// Hold every session at once and let four workers each advance a separate shard.
pub(super) fn run(
    factory: &LoadedGame,
    config: &ComponentValue,
    action: &ComponentValue,
) -> io::Result<()> {
    let baseline = memory()?;
    println!(
        "baseline rss_mib={:.1} footprint_mib={:.1}",
        baseline.resident_size as f64 / MIB,
        baseline.phys_footprint as f64 / MIB,
    );
    for games in [16, 64, 128, 256] {
        let mut sessions = Vec::with_capacity(games);
        let init_started = Instant::now();
        for index in 0..games {
            let game = factory.session();
            let state = game.init(config, 0).unwrap();
            sessions.push((game, state));
            if (index + 1) % 16 == 0 && memory()?.phys_footprint >= MAX_FOOTPRINT {
                println!(
                    "Stopped at {} initializations at the footprint budget",
                    index + 1
                );
                return Ok(());
            }
        }
        let init_ms = init_started.elapsed().as_secs_f64() * 1_000.0;
        let ready = memory()?;
        let barrier = Barrier::new(WORKERS + 1);
        let mut rounds = Vec::with_capacity(TURNS);
        let mut active = None;
        thread::scope(|scope| {
            let mut workers = Vec::with_capacity(WORKERS);
            for shard in sessions.chunks_mut(games / WORKERS) {
                let barrier = &barrier;
                workers.push(scope.spawn(move || {
                    let mut failure = None;
                    for turn in 0..TURNS {
                        let actor = if turn % 2 == 0 {
                            PlayerRole::A
                        } else {
                            PlayerRole::B
                        };
                        barrier.wait();
                        if failure.is_none() {
                            for (game, state) in shard.iter_mut() {
                                match game.step(state, action, actor) {
                                    Ok(Ok(next)) => *state = next,
                                    Ok(Err(error)) => failure = Some(error.to_string()),
                                    Err(error) => failure = Some(error.to_string()),
                                }
                                if failure.is_some() {
                                    break;
                                }
                            }
                        }
                        barrier.wait();
                    }
                    failure
                }));
            }
            for turn in 0..TURNS {
                let before = Instant::now();
                barrier.wait();
                barrier.wait();
                rounds.push(before.elapsed().as_secs_f64());
                if turn == TURNS / 2 {
                    active = Some(memory());
                }
            }
            for worker in workers {
                if let Some(error) = worker.join().expect("stress worker panicked") {
                    return Err(io::Error::other(error));
                }
            }
            Ok(())
        })?;
        let active = active.expect("midgame round must have completed")?;
        for (_, state) in &sessions {
            assert_eq!(
                serde_json::to_value(state.completion().expect("expected a winner")).unwrap(),
                serde_json::json!({"winner": "B"}),
            );
        }
        let rate = (games * TURNS) as f64 / rounds.iter().sum::<f64>();
        let slowest = rounds.into_iter().max_by(f64::total_cmp).unwrap() * 1_000.0;
        println!(
            "{games} games init_ms={init_ms:.2} ready_rss_mib={:.1} ready_footprint_mib={:.1} \
             active_rss_mib={:.1} active_footprint_mib={:.1} turns_per_s={rate:.0} slowest_round_ms={slowest:.2}",
            ready.resident_size as f64 / MIB,
            ready.phys_footprint as f64 / MIB,
            active.resident_size as f64 / MIB,
            active.phys_footprint as f64 / MIB,
        );
        if active.phys_footprint >= MAX_FOOTPRINT {
            println!("Stopped after the stage reached the footprint budget");
            return Ok(());
        }
    }
    Ok(())
}
