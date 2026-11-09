//! C interface of `md-env`, for Python's `ctypes` (python/megadrive_env).
//!
//! Contract for every function taking pointers (checked by the Python
//! wrapper, which is the only intended caller):
//! - `env` comes from `mdenv_sonic1_new` and was not freed yet; one thread
//!   uses a given `env` at a time;
//! - `rom` / `state` point to `len` readable bytes;
//! - `features_out` has room for `mdenv_feature_count()` floats,
//!   `gray_out` for `mdenv_gray_side()²` bytes, `info_out` for
//!   `mdenv_info_count()` 64-bit integers; output pointers may be null when
//!   the caller does not want that output.
//!
//! A Rust panic must not unwind into C: every function catches it and
//! returns an error code (null, or -1).

use std::panic::{AssertUnwindSafe, catch_unwind};

use md_env::frame::{GRAY_SIDE, GRAY_SIZE};
use md_env::{End, Env, EnvConfig, FeatureSet, Game, RewardMode, Sonic1};

/// An environment, opaque to C.
pub struct MdEnv(Env<Sonic1>);

/// Flags returned by `mdenv_step`.
pub const FLAG_DIED: i32 = 1 << 0;
pub const FLAG_COMPLETED: i32 = 1 << 1;
pub const FLAG_TRUNCATED: i32 = 1 << 2;
/// Error code of the functions returning an `i32`.
pub const ERROR: i32 = -1;
/// Version of this interface, checked by the Python wrapper (a library built
/// from another version would be called with the wrong arguments). Raised
/// on every change of the functions, added ones included.
pub const API_VERSION: u32 = 8;

/// Sonic 1 variables given by `mdenv_info`, in this order: x, y, speed_x,
/// speed_y, camera_x, limit_right, zone, act, lives, rings, score, inertia,
/// angle, status, jumping, joypad_held.
pub const INFO_COUNT: usize = 16;

/// Runs `f`, turning a panic into `fallback` (it must not cross into C).
fn guarded<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        eprintln!("md-env-c: internal error (panic caught)");
        fallback
    })
}

/// Copies the current features into `out` when it is not null.
///
/// # Safety
/// `out` is null or has room for `env.features().len()` floats.
unsafe fn write_features(env: &Env<Sonic1>, out: *mut f32) {
    if !out.is_null() {
        let features = env.features();
        // SAFETY: room for the env's feature count (module contract);
        // `features` has that many elements and does not overlap C's buffer.
        unsafe { std::ptr::copy_nonoverlapping(features.as_ptr(), out, features.len()) };
    }
}

/// Creates a Sonic 1 environment for the ROM image `rom[..len]`, holding
/// each action `frame_skip` frames, cut after `max_steps` steps or after
/// `stall_steps` steps without a new best X (0: never), rewarding each
/// pixel (`reward_mode` 0) or only new ground (1), giving the basic numbers
/// (`feature_set` 0), the extended ones (1), the full ones (2) or the full
/// ones with the 2 nearest dangers (3) or the 3 nearest, spikes included
/// (4), taking `jump_penalty` off
/// the reward at each jump (0: jumps are free; negative or not finite is an
/// error). Returns null on error (message on stderr).
///
/// # Safety
/// `rom` points to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_sonic1_new(
    rom: *const u8,
    len: usize,
    frame_skip: u32,
    max_steps: u64,
    stall_steps: u64,
    reward_mode: u32,
    feature_set: u32,
    jump_penalty: f32,
) -> *mut MdEnv {
    if rom.is_null() {
        return std::ptr::null_mut();
    }
    let reward_mode = match reward_mode {
        0 => RewardMode::Delta,
        1 => RewardMode::Best,
        _ => {
            eprintln!("md-env-c: reward mode {reward_mode} (0 delta, 1 best)");
            return std::ptr::null_mut();
        }
    };
    let features = match feature_set {
        0 => FeatureSet::Basic,
        1 => FeatureSet::Extended,
        2 => FeatureSet::Full,
        3 => FeatureSet::Objects,
        4 => FeatureSet::Hazards,
        _ => {
            eprintln!(
                "md-env-c: feature set {feature_set} (0 basic, 1 extended, 2 full, 3 objects, 4 hazards)"
            );
            return std::ptr::null_mut();
        }
    };
    if !(jump_penalty.is_finite() && jump_penalty >= 0.0) {
        eprintln!("md-env-c: jump penalty {jump_penalty} (0 or more)");
        return std::ptr::null_mut();
    }
    // SAFETY: `rom` points to `len` readable bytes (contract), only read here.
    let rom = unsafe { std::slice::from_raw_parts(rom, len) };
    guarded(std::ptr::null_mut(), || {
        let config = EnvConfig {
            frame_skip,
            max_steps: (max_steps > 0).then_some(max_steps),
            stall_steps: (stall_steps > 0).then_some(stall_steps),
            ..EnvConfig::default()
        };
        let game = Sonic1 {
            reward_mode,
            features,
            jump_penalty,
            ..Sonic1::default()
        };
        match Env::new(rom, game, config) {
            Ok(env) => Box::into_raw(Box::new(MdEnv(env))),
            Err(e) => {
                eprintln!("md-env-c: {e}");
                std::ptr::null_mut()
            }
        }
    })
}

/// Makes `state[..len]` (a save state of the same ROM) the start of every
/// episode and resets to it. 0 if accepted, -1 otherwise (the environment
/// is then unchanged).
///
/// # Safety
/// `env` is live; `state` points to `len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_set_start_state(
    env: *mut MdEnv,
    state: *const u8,
    len: usize,
) -> i32 {
    if env.is_null() || state.is_null() {
        return ERROR;
    }
    // SAFETY: live env used by one thread, readable state (contract).
    let (env, state) = unsafe { (&mut (*env).0, std::slice::from_raw_parts(state, len)) };
    guarded(ERROR, || match env.set_start_state(state.to_vec()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("md-env-c: {e}");
            ERROR
        }
    })
}

/// Writes a save state of the machine as it is now into `out[..capacity]`
/// and returns its size; with a null `out` or too small a `capacity`, only
/// returns the size needed (call again with a buffer that large). -1 on
/// error.
///
/// # Safety
/// `env` is live; `out` is null or has `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_save_state(env: *mut MdEnv, out: *mut u8, capacity: usize) -> i64 {
    if env.is_null() {
        return i64::from(ERROR);
    }
    // SAFETY: live env used by one thread (contract).
    let env = unsafe { &mut (*env).0 };
    guarded(i64::from(ERROR), || {
        let state = env.save_state();
        if !out.is_null() && capacity >= state.len() {
            // SAFETY: `out` has `capacity` >= state.len() writable bytes.
            unsafe { std::ptr::copy_nonoverlapping(state.as_ptr(), out, state.len()) };
        }
        state.len() as i64
    })
}

/// Begins an episode; writes the features into `features_out`.
///
/// # Safety
/// `env` is live; `features_out` is null or large enough.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_reset(env: *mut MdEnv, features_out: *mut f32) -> i32 {
    if env.is_null() {
        return ERROR;
    }
    // SAFETY: live env used by one thread (contract).
    let env = unsafe { &mut (*env).0 };
    guarded(ERROR, || {
        env.reset();
        // SAFETY: features_out null or large enough (contract).
        unsafe { write_features(env, features_out) };
        0
    })
}

/// Plays action `action` (0..`mdenv_action_count()`); writes the reward and
/// the features. Returns the flags (`FLAG_DIED`, `FLAG_COMPLETED`,
/// `FLAG_TRUNCATED`; 0 while the episode goes on), or -1 for an action out
/// of range.
///
/// # Safety
/// `env` is live; `reward_out` and `features_out` are null or large enough.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_step(
    env: *mut MdEnv,
    action: u32,
    reward_out: *mut f32,
    features_out: *mut f32,
) -> i32 {
    if env.is_null() || action as usize >= Sonic1::ACTIONS.len() {
        return ERROR;
    }
    // SAFETY: live env used by one thread (contract).
    let env = unsafe { &mut (*env).0 };
    guarded(ERROR, || {
        let result = env.step(action as usize);
        if !reward_out.is_null() {
            // SAFETY: reward_out points to one writable float (contract).
            unsafe { *reward_out = result.reward };
        }
        // SAFETY: features_out null or large enough (contract).
        unsafe { write_features(env, features_out) };
        let mut flags = 0;
        match result.end {
            Some(End::Died) => flags |= FLAG_DIED,
            Some(End::Completed) => flags |= FLAG_COMPLETED,
            None => {}
        }
        if result.truncated {
            flags |= FLAG_TRUNCATED;
        }
        flags
    })
}

/// Writes the picture as `mdenv_gray_side()`² grey levels into `gray_out`.
///
/// # Safety
/// `env` is live; `gray_out` has room for the picture.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_observe_gray(env: *const MdEnv, gray_out: *mut u8) -> i32 {
    if env.is_null() || gray_out.is_null() {
        return ERROR;
    }
    // SAFETY: live env; gray_out has GRAY_SIZE writable bytes (contract).
    let (env, out) = unsafe {
        (
            &(*env).0,
            std::slice::from_raw_parts_mut(gray_out, GRAY_SIZE),
        )
    };
    guarded(ERROR, || {
        env.observe_gray(out);
        0
    })
}

/// Writes Sonic 1's variables (see `INFO_COUNT`) into `info_out`.
///
/// # Safety
/// `env` is live; `info_out` has room for `INFO_COUNT` integers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_info(env: *const MdEnv, info_out: *mut i64) -> i32 {
    if env.is_null() || info_out.is_null() {
        return ERROR;
    }
    // SAFETY: live env; info_out has INFO_COUNT writable integers (contract).
    let (env, out) = unsafe {
        (
            &(*env).0,
            std::slice::from_raw_parts_mut(info_out, INFO_COUNT),
        )
    };
    let s = env.state();
    out.copy_from_slice(&[
        i64::from(s.x),
        i64::from(s.y),
        i64::from(s.speed_x),
        i64::from(s.speed_y),
        i64::from(s.camera_x),
        i64::from(s.limit_right),
        i64::from(s.zone),
        i64::from(s.act),
        i64::from(s.lives),
        i64::from(s.rings),
        i64::from(s.score),
        i64::from(s.inertia),
        i64::from(s.angle),
        i64::from(s.status),
        i64::from(s.jumping),
        i64::from(s.joypad_held),
    ]);
    0
}

/// Frees an environment (null is accepted and ignored).
///
/// # Safety
/// `env` is null or live, and not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_free(env: *mut MdEnv) {
    if !env.is_null() {
        // SAFETY: `env` came from Box::into_raw in mdenv_sonic1_new and is
        // freed once (contract).
        drop(unsafe { Box::from_raw(env) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn mdenv_api_version() -> u32 {
    API_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn mdenv_action_count() -> u32 {
    Sonic1::ACTIONS.len() as u32
}

/// Numbers in `env`'s feature vector (0 for null).
///
/// # Safety
/// `env` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mdenv_feature_count(env: *const MdEnv) -> u32 {
    if env.is_null() {
        return 0;
    }
    // SAFETY: live env (contract).
    unsafe { (*env).0.features().len() as u32 }
}

#[unsafe(no_mangle)]
pub extern "C" fn mdenv_gray_side() -> u32 {
    GRAY_SIDE as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn mdenv_info_count() -> u32 {
    INFO_COUNT as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use md_core::testrom::pad_reader;

    /// A live environment on the pad-reading test ROM.
    fn new(max_steps: u64) -> *mut MdEnv {
        let rom = pad_reader();
        // SAFETY: rom is a live buffer of rom.len() bytes.
        let env = unsafe { mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, max_steps, 0, 0, 0, 0.0) };
        assert!(!env.is_null());
        env
    }

    #[test]
    fn sizes_are_given_to_the_caller() {
        assert_eq!(mdenv_action_count(), 7);

        assert_eq!(mdenv_gray_side(), 84);
        assert_eq!(mdenv_info_count(), 16);
        assert_eq!(mdenv_api_version(), API_VERSION);
    }

    #[test]
    fn reset_step_and_flags() {
        let env = new(2);
        let mut features = [9.0f32; 6];
        // SAFETY: live env.
        assert_eq!(unsafe { mdenv_feature_count(env) }, 6);
        let mut reward = 9.0f32;
        let mut gray = vec![0u8; 84 * 84];
        let mut info = [-1i64; INFO_COUNT];
        // SAFETY: live env and buffers of the documented sizes.
        unsafe {
            assert_eq!(mdenv_reset(env, features.as_mut_ptr()), 0);
            assert_eq!(mdenv_step(env, 1, &mut reward, features.as_mut_ptr()), 0);
            // A test ROM has none of Sonic's variables: zeros, no reward.
            assert_eq!(reward, 0.0);
            assert_eq!(features, [0.0; 6]);
            assert_eq!(
                mdenv_step(env, 0, &mut reward, std::ptr::null_mut()),
                FLAG_TRUNCATED
            );
            assert_eq!(mdenv_observe_gray(env, gray.as_mut_ptr()), 0);
            assert_eq!(mdenv_info(env, info.as_mut_ptr()), 0);
            assert_eq!(info, [0; INFO_COUNT]);
            assert_eq!(
                mdenv_reset(env, std::ptr::null_mut()),
                0,
                "null outputs accepted"
            );
            mdenv_free(env);
        }
    }

    #[test]
    fn errors_are_codes_not_crashes() {
        let env = new(0);
        let mut reward = 0.0f32;
        // SAFETY: live env; garbage state is a readable buffer.
        unsafe {
            assert_eq!(
                mdenv_step(env, 7, &mut reward, std::ptr::null_mut()),
                ERROR,
                "action out of range"
            );
            let garbage = [1u8, 2, 3];
            assert_eq!(
                mdenv_set_start_state(env, garbage.as_ptr(), garbage.len()),
                ERROR
            );
            assert_eq!(
                mdenv_step(env, 0, &mut reward, std::ptr::null_mut()),
                0,
                "still usable"
            );
            assert_eq!(
                mdenv_step(std::ptr::null_mut(), 0, &mut reward, std::ptr::null_mut()),
                ERROR
            );
            assert_eq!(mdenv_observe_gray(env, std::ptr::null_mut()), ERROR);
            mdenv_free(env);
            mdenv_free(std::ptr::null_mut());
            let not_a_rom = [0u8; 16];
            assert!(
                mdenv_sonic1_new(not_a_rom.as_ptr(), not_a_rom.len(), 4, 0, 0, 0, 0, 0.0).is_null()
            );
            assert!(mdenv_sonic1_new(std::ptr::null(), 0, 4, 0, 0, 0, 0, 0.0).is_null());
            let rom = pad_reader();
            assert!(
                mdenv_sonic1_new(rom.as_ptr(), rom.len(), 0, 0, 0, 0, 0, 0.0).is_null(),
                "frame_skip 0"
            );
        }
    }

    #[test]
    fn stall_cut_and_reward_mode() {
        let rom = pad_reader();
        // SAFETY: rom is a live buffer; env is live until freed.
        unsafe {
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 2, 1, 0, 0.0);
            assert!(!env.is_null(), "stall 2, best mode");
            mdenv_reset(env, std::ptr::null_mut());
            // A test ROM never moves Sonic's X: cut after two steps.
            let flags: Vec<i32> = (0..2)
                .map(|_| mdenv_step(env, 0, std::ptr::null_mut(), std::ptr::null_mut()))
                .collect();
            assert_eq!(flags, [0, FLAG_TRUNCATED]);
            mdenv_free(env);
            assert!(
                mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 2, 0, 0.0).is_null(),
                "unknown reward mode"
            );
        }
    }

    #[test]
    fn extended_features_per_environment() {
        let rom = pad_reader();
        // SAFETY: rom is a live buffer; env is live until freed.
        unsafe {
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 1, 0.0);
            assert_eq!(mdenv_feature_count(env), 11);
            let mut features = [9.0f32; 11];
            assert_eq!(mdenv_reset(env, features.as_mut_ptr()), 0);
            assert_eq!(features[8], 1.0, "angle 0: cosine 1 (all 11 written)");
            mdenv_free(env);
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 2, 0.0);
            assert_eq!(mdenv_feature_count(env), 12);
            let mut features = [9.0f32; 12];
            assert_eq!(mdenv_reset(env, features.as_mut_ptr()), 0);
            assert_eq!(
                features[11], 0.0,
                "no button held at power-on (all 12 written)"
            );
            mdenv_free(env);
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 3, 0.0);
            assert_eq!(mdenv_feature_count(env), 20);
            mdenv_free(env);
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 4, 0.0);
            assert_eq!(mdenv_feature_count(env), 24);
            mdenv_free(env);
            assert!(mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 5, 0.0).is_null());
            assert!(
                mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 0, -0.5).is_null(),
                "negative jump penalty"
            );
            assert!(
                mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 0, f32::NAN).is_null(),
                "not a number"
            );
            let env = mdenv_sonic1_new(rom.as_ptr(), rom.len(), 4, 0, 0, 0, 0, 0.5);
            assert!(!env.is_null(), "a jump penalty");
            mdenv_free(env);
            assert_eq!(mdenv_feature_count(std::ptr::null()), 0);
        }
    }

    #[test]
    fn a_saved_state_becomes_the_start() {
        let env = new(0);
        // SAFETY: live env, buffers of the documented sizes.
        unsafe {
            mdenv_reset(env, std::ptr::null_mut());
            for _ in 0..3 {
                mdenv_step(env, 0, std::ptr::null_mut(), std::ptr::null_mut());
            }
            let size = mdenv_save_state(env, std::ptr::null_mut(), 0);
            assert!(size > 0, "size asked first");
            let mut state = vec![0u8; size as usize];
            assert_eq!(mdenv_save_state(env, state.as_mut_ptr(), state.len()), size);
            let frame = (*env).0.console().frame_count();
            assert_eq!(mdenv_set_start_state(env, state.as_ptr(), state.len()), 0);
            mdenv_step(env, 1, std::ptr::null_mut(), std::ptr::null_mut());
            mdenv_reset(env, std::ptr::null_mut());
            assert_eq!((*env).0.console().frame_count(), frame);
            mdenv_free(env);
        }
    }
}
