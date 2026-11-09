//! Sonic the Hedgehog (Mega Drive, 1991): where its state lives in work RAM,
//! the buttons an agent presses, and a reward for going right.
//!
//! Sources of the addresses (68000 work RAM, `0xFF0000`-based):
//! - Gym Retro, `retro/data/stable/SonicTheHedgehog-Genesis/data.json`
//!   (github.com/openai/retro), the variables of OpenAI's Sonic contest;
//! - TASVideos, "GameResources/Genesis/SonicTheHedgehog": X, Y, speeds,
//!   camera;
//! - the community disassembly, `_Variables.asm`
//!   (github.com/sonicretro/s1disasm): the names and sizes
//!   (`v_screenposx` `ds.l`, `v_limitright2` `ds.w`, `v_lives` `ds.b`...).
//!
//! Only X, Y and the camera are confirmed by two independent sources; the
//! speeds come from TASVideos alone and the rest from Gym Retro (the
//! disassembly confirms what the variables mean). All were then checked on
//! the running game with `examples/bench.rs --watch` (docs/decisions.md):
//! X against the camera, rings, lives, speeds, the death and the act change
//! at the end of Green Hill Zone 1, and the reward sums.

use md_core::Buttons;

use crate::game::{End, Game, ram_u8, ram_u16, ram_u32};

/// Addresses in the 68000 address space.
pub mod addr {
    /// Player X, pixels (word). Gym Retro `x` reads it signed, TASVideos
    /// unsigned (it can wrap): read as unsigned here.
    pub const X: u32 = 0xFF_D008;
    /// Player Y, pixels (word).
    pub const Y: u32 = 0xFF_D00C;
    /// Player X speed (signed word, 1/256 pixel per frame). TASVideos.
    pub const SPEED_X: u32 = 0xFF_D010;
    /// Player Y speed (signed word). TASVideos.
    pub const SPEED_Y: u32 = 0xFF_D012;
    /// Camera X: the pixel word of the long `v_screenposx`.
    pub const CAMERA_X: u32 = 0xFF_F700;
    /// Right limit of the camera, pixels (`v_limitright2`, "right level
    /// boundary"). Seen in Green Hill Zone 1: 9407; the camera stops there
    /// while Sonic runs on past the signpost (x 9767), so `x / limit` ends a
    /// little above 1.
    pub const LIMIT_RIGHT: u32 = 0xFF_F72A;
    pub const ZONE: u32 = 0xFF_FE10;
    pub const ACT: u32 = 0xFF_FE11;
    pub const LIVES: u32 = 0xFF_FE12;
    pub const RINGS: u32 = 0xFF_FE20;
    pub const SCORE: u32 = 0xFF_FE26;
    /// Sonic's object routine: `v_player` ($FFD000, the object holding X
    /// at `obX` = 8) + `obRoutine` ($24, _Constants.asm). Values from
    /// `Sonic_Index` (_incObj/01 Sonic.asm): 0 Main, 2 Control, 4 Hurt,
    /// 6 Death, 8 ResetLevel. Seen on the game: 2 → 6 at the fatal hit,
    /// 19 steps before the lives drop.
    pub const ROUTINE: u32 = 0xFF_D024;
    /// `f_timecount`, "time counter update flag": its address follows from
    /// the declaration order in _Variables.asm, from `v_zone` ($FFFE10) to
    /// `v_rings` ($FFFE20), both checked. Seen on the game: 1 while playing,
    /// 0 when the timer stops as Sonic passes the signpost, about 170 steps
    /// before the act changes.
    pub const TIME_COUNT: u32 = 0xFF_FE1E;
    /// Ground speed (signed word): `obInertia equ $14`, "potential speed"
    /// (_Constants.asm); TASVideos "Speed (on ground), FFD014". What carries
    /// Sonic around a loop, where the X speed changes sign.
    pub const INERTIA: u32 = 0xFF_D014;
    /// Angle of the floor under Sonic (byte, 256 for a full turn): `obAngle
    /// equ $26` (_Constants.asm).
    pub const ANGLE: u32 = 0xFF_D026;
    /// `obStatus equ $22` (_Constants.asm): bit 1 `status_in_air_bit`
    /// ("Sonic is in the air", also `btst #1,$22(a0)` in Sonic Retro's
    /// guides), bit 2 `status_rolling_bit` ("in ball form").
    pub const STATUS: u32 = 0xFF_D022;
    /// Jump flag (byte): `jumping: equ $3C ; flag set while Sonic is
    /// jumping` (_Constants.asm, Sonic's object variables). `Sonic_Jump`
    /// sets it to 1 only when A, B or C was just pressed (`v_jpadpress2`);
    /// `Sonic_ResetOnFloor` clears it on landing (_incObj/01 Sonic.asm).
    /// Falling off a ledge sets the in-air bit, not this flag.
    pub const JUMPING: u32 = 0xFF_D03C;
    /// Buttons held, as the game sees them (byte): `v_jpadhold2 ; joypad
    /// input - held, duplicate`, the copy Sonic's code reads. Its address
    /// follows from the declaration order in _Variables.asm: `v_gamemode`
    /// (1 byte), 1 unused, `v_jpadhold2`, `v_jpadpress2`, `v_jpadhold1`,
    /// `v_jpadpress1`, 6 unused, `v_vdp_buffer1` (word), 6 unused,
    /// `v_generictimer` (word), then `v_scrposy_vdp`, which TASVideos gives
    /// as $FFF616 (camera Y "also at F616"): counting back puts
    /// `v_gamemode` at $FFF600 and this at $FFF602.
    pub const JOYPAD_HELD: u32 = 0xFF_F602;
    /// Level objects: `v_lvlobjspace equ v_objspace+object_size*32`, 96
    /// slots of `object_size` ($40) bytes (_Variables.asm, _Constants.asm).
    /// `v_objspace` is $FFD000: `v_player` is its slot 0, and Sonic's X
    /// (`obX` = 8) was checked at $FFD008.
    pub const LEVEL_OBJECTS: u32 = 0xFF_D800;
}

/// The level objects as `Sonic_ReactToItem` (_incObj/Sonic ReactToItem.asm)
/// walks them: every slot of `addr::LEVEL_OBJECTS`, skipping those not on
/// screen (`tst.b obRender(a1)`, `bpl.s React_CheckNext`).
pub mod object {
    /// Slots in level object RAM (`v_lvlobjend - v_lvlobjspace`).
    pub const COUNT: u32 = 96;
    /// `object_size`.
    pub const SIZE: u32 = 0x40;
    /// Offsets in a slot (_Constants.asm).
    pub const ID: u32 = 0x00;
    pub const RENDER: u32 = 0x01;
    pub const X: u32 = 0x08;
    pub const Y: u32 = 0x0C;
    pub const COL_TYPE: u32 = 0x20;
    /// `sprite_rendered_bit` (7) of `obRender`: "set when sprite is in
    /// visible screen space and got rendered the previous frame".
    pub const RENDERED: u8 = 0x80;
    /// `obColType` category, `andi.b #col_item|col_hurt|col_special`: $00-$3F
    /// a badnik (or a boss), $40-$7F an item (rings, monitors), $80-$BF
    /// harmful (spikes and the like), $C0 and above special. 0 is no
    /// collision.
    pub const CATEGORY: u8 = 0xC0;
    pub const CATEGORY_BADNIK: u8 = 0x00;
    pub const CATEGORY_HURT: u8 = 0x80;
    /// `id_Spikes` ($36, _inc/Object Pointers.asm). Spikes never set
    /// `obColType`: they are solid (`SolidObject`) and call `HurtSonic`
    /// themselves (_incObj/36 Spikes.asm, `Spikes_Hurt`), so the category
    /// above misses them.
    pub const ID_SPIKES: u8 = 0x36;
}

/// How many dangers (badniks, harmful objects) `FeatureSet::Objects` tells
/// the agent about.
pub const DANGERS: usize = 2;
/// How many `FeatureSet::Hazards` tells about (spikes included: they often
/// come in rows), and the length of `SonicState::dangers`.
pub const HAZARDS: usize = 3;

/// A dangerous object on screen, relative to Sonic.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Danger {
    /// Object X and Y minus Sonic's, pixels.
    pub dx: i16,
    pub dy: i16,
    /// A badnik (it can be destroyed by jumping or rolling into it), else
    /// harmful whatever Sonic does.
    pub badnik: bool,
}

/// The `HAZARDS` dangers on screen nearest to Sonic at (`x`, `y`), nearest
/// first; spikes count only if `spikes`.
fn dangers(ram: &[u8], x: u16, y: u16, spikes: bool) -> [Option<Danger>; HAZARDS] {
    let mut found: [Option<(i32, Danger)>; HAZARDS] = [None; HAZARDS];
    for slot in 0..object::COUNT {
        let base = addr::LEVEL_OBJECTS + slot * object::SIZE;
        let col = ram_u8(ram, base + object::COL_TYPE);
        let category = col & object::CATEGORY;
        let badnik = category == object::CATEGORY_BADNIK && col != 0;
        let dangerous = badnik
            || category == object::CATEGORY_HURT
            || (spikes && ram_u8(ram, base + object::ID) == object::ID_SPIKES);
        if !dangerous || ram_u8(ram, base + object::RENDER) & object::RENDERED == 0 {
            continue;
        }
        let danger = Danger {
            dx: ram_u16(ram, base + object::X).wrapping_sub(x) as i16,
            dy: ram_u16(ram, base + object::Y).wrapping_sub(y) as i16,
            badnik,
        };
        let distance = i32::from(danger.dx).pow(2) + i32::from(danger.dy).pow(2);
        // Insertion into the short sorted list.
        let mut entry = Some((distance, danger));
        for place in found.iter_mut() {
            match (*place, entry) {
                (None, _) => {
                    *place = entry;
                    break;
                }
                (Some((d, _)), Some((e, _))) if e < d => {
                    entry = place.replace(entry.unwrap());
                }
                _ => {}
            }
        }
    }
    found.map(|f| f.map(|(_, danger)| danger))
}

/// `obStatus` bits (`addr::STATUS`).
pub const STATUS_IN_AIR: u8 = 1 << 1;
pub const STATUS_ROLLING: u8 = 1 << 2;
/// A, B and C in the joypad bytes (`btnABC`, $70, _Constants.asm): any of
/// them jumps, on the press only (`Sonic_Jump` tests `v_jpadpress2`).
pub const BUTTONS_JUMP: u8 = 0x70;

/// Which numbers an agent reads (`Game::features`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeatureSet {
    /// Progress, height, position on the screen, both speeds, rings.
    #[default]
    Basic,
    /// Basic, then what a loop needs: ground speed, the floor angle (sine and
    /// cosine: no jump where it wraps), in the air, rolling.
    Extended,
    /// Extended, then whether a jump button is held. Sonic jumps only when
    /// one is pressed: right + jump after right jumps, after right + jump it
    /// just runs. Without this number the agent cannot tell which.
    Full,
    /// Full, then the `DANGERS` nearest dangers on screen, 4 numbers each:
    /// present (0 or 1), dx / 320, dy / 224 (one screen), badnik (0 or 1).
    Objects,
    /// Full, then the `HAZARDS` nearest dangers, spikes included, in the
    /// same 4 numbers. Objects misses spikes (`object::ID_SPIKES`); a
    /// separate set, so that models trained on Objects still see the same.
    Hazards,
}

impl FeatureSet {
    pub const fn count(self) -> usize {
        match self {
            FeatureSet::Basic => 6,
            FeatureSet::Extended => 11,
            FeatureSet::Full => 12,
            FeatureSet::Objects => 12 + 4 * DANGERS,
            FeatureSet::Hazards => 12 + 4 * HAZARDS,
        }
    }
}

/// `obRoutine` of Sonic from which he is dead (Sonic_Death, then
/// Sonic_ResetLevel). 4 (Sonic_Hurt) is a hit with rings: not a death.
pub const ROUTINE_DEATH: u8 = 6;

/// The variables of the game at one moment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SonicState {
    pub x: u16,
    pub y: u16,
    pub speed_x: i16,
    pub speed_y: i16,
    pub camera_x: u16,
    pub limit_right: u16,
    pub zone: u8,
    pub act: u8,
    pub lives: u8,
    pub rings: u16,
    pub score: u32,
    /// Sonic's object routine (`addr::ROUTINE`).
    pub routine: u8,
    /// Time counter running (`addr::TIME_COUNT`).
    pub time_count: u8,
    /// Ground speed (`addr::INERTIA`).
    pub inertia: i16,
    /// Floor angle (`addr::ANGLE`).
    pub angle: u8,
    /// Object status bits (`addr::STATUS`).
    pub status: u8,
    /// Jump flag (`addr::JUMPING`).
    pub jumping: u8,
    /// Buttons held (`addr::JOYPAD_HELD`).
    pub joypad_held: u8,
    /// Nearest dangers on screen, nearest first (spikes included only with
    /// `FeatureSet::Hazards`).
    pub dangers: [Option<Danger>; HAZARDS],
}

impl SonicState {
    pub fn dead(&self) -> bool {
        self.routine >= ROUTINE_DEATH
    }
}

/// How going right is rewarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RewardMode {
    /// Every pixel right gains, every pixel left loses.
    #[default]
    Delta,
    /// Only new ground gains: X beyond the best reached in the episode
    /// (going back then forth again gives nothing). The principle of
    /// OpenAI's Sonic contest reward.
    Best,
}

/// What the reward remembers in an episode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SonicMemory {
    /// Best X so far (`RewardMode::Best`); set from the first step's start.
    pub best_x: Option<u16>,
}

/// Reward weights; the defaults are a first guess, to tune while training.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sonic1 {
    /// Reward per pixel gone right (negative going left). Green Hill Zone 1
    /// (x 80 to 9767) gives about 97 with the default.
    pub progress_scale: f32,
    /// Reward when the zone or the act changes: the level was finished.
    pub completion_bonus: f32,
    /// Subtracted when a life is lost.
    pub death_penalty: f32,
    pub reward_mode: RewardMode,
    pub features: FeatureSet,
    /// Subtracted at each jump (the jump flag going up). 0 leaves jumps
    /// free: an agent then jumps at random, even inside a loop, where a
    /// jump throws Sonic off the track.
    pub jump_penalty: f32,
}

impl Default for Sonic1 {
    fn default() -> Self {
        Sonic1 {
            progress_scale: 0.01,
            completion_bonus: 50.0,
            death_penalty: 10.0,
            reward_mode: RewardMode::Delta,
            features: FeatureSet::Basic,
            jump_penalty: 0.0,
        }
    }
}

/// Sonic died between `before` and `now`: his routine became Death (at the
/// fatal hit), or a life was lost (later, after the death animation; kept
/// as a second signal).
fn died(before: &SonicState, now: &SonicState) -> bool {
    (now.dead() && !before.dead()) || now.lives < before.lives
}

/// The level was finished between `before` and `now`: the timer stopped
/// with Sonic alive (at the signpost), or the zone or act changed (later,
/// after the score tally; kept as a second signal). The timer also stops
/// when Sonic dies, hence the alive condition.
fn finished(before: &SonicState, now: &SonicState) -> bool {
    (before.time_count != 0 && now.time_count == 0 && !now.dead())
        || (now.zone, now.act) != (before.zone, before.act)
}

/// Normalisation of the feature vector (round numbers, not game limits).
const FEATURE_Y_RANGE: f32 = 2048.0;
const FEATURE_SCREEN_WIDTH: f32 = 320.0;
const FEATURE_SCREEN_HEIGHT: f32 = 224.0;
const FEATURE_SPEED_RANGE: f32 = 4096.0;
const FEATURE_RINGS_CAP: u16 = 100;

impl Game for Sonic1 {
    type State = SonicState;
    type Memory = SonicMemory;

    /// Nothing, right, right + jump, jump, down (roll or crouch), left,
    /// left + jump. Sonic jumps with B (any of A, B, C does).
    const ACTIONS: &'static [Buttons] = &[
        Buttons::NONE,
        Buttons::RIGHT,
        Buttons::RIGHT.union(Buttons::B),
        Buttons::B,
        Buttons::DOWN,
        Buttons::LEFT,
        Buttons::LEFT.union(Buttons::B),
    ];

    fn read(&self, ram: &[u8]) -> SonicState {
        let (x, y) = (ram_u16(ram, addr::X), ram_u16(ram, addr::Y));
        SonicState {
            x,
            y,
            speed_x: ram_u16(ram, addr::SPEED_X) as i16,
            speed_y: ram_u16(ram, addr::SPEED_Y) as i16,
            camera_x: ram_u16(ram, addr::CAMERA_X),
            limit_right: ram_u16(ram, addr::LIMIT_RIGHT),
            zone: ram_u8(ram, addr::ZONE),
            act: ram_u8(ram, addr::ACT),
            lives: ram_u8(ram, addr::LIVES),
            rings: ram_u16(ram, addr::RINGS),
            score: ram_u32(ram, addr::SCORE),
            routine: ram_u8(ram, addr::ROUTINE),
            time_count: ram_u8(ram, addr::TIME_COUNT),
            inertia: ram_u16(ram, addr::INERTIA) as i16,
            angle: ram_u8(ram, addr::ANGLE),
            status: ram_u8(ram, addr::STATUS),
            jumping: ram_u8(ram, addr::JUMPING),
            joypad_held: ram_u8(ram, addr::JOYPAD_HELD),
            dangers: dangers(ram, x, y, self.features == FeatureSet::Hazards),
        }
    }

    fn progress(&self, state: &SonicState) -> i64 {
        i64::from(state.x)
    }

    fn reward(&self, memory: &mut SonicMemory, prev: &SonicState, now: &SonicState) -> f32 {
        // The bonus and the penalty come with the step that ends the
        // episode (`end`): the same signals, seen from the previous step.
        if finished(prev, now) {
            // X may start over in a new level: its change means nothing.
            return self.completion_bonus;
        }
        if died(prev, now) {
            return -self.death_penalty;
        }
        let jump = if prev.jumping == 0 && now.jumping != 0 {
            self.jump_penalty
        } else {
            0.0
        };
        // Wrapping differences as signed: a pixel count that wraps still
        // reads as a small step.
        let progress = match self.reward_mode {
            RewardMode::Delta => f32::from(now.x.wrapping_sub(prev.x) as i16) * self.progress_scale,
            RewardMode::Best => {
                let best = memory.best_x.get_or_insert(prev.x);
                let gain = now.x.wrapping_sub(*best) as i16;
                if gain > 0 {
                    *best = now.x;
                    f32::from(gain) * self.progress_scale
                } else {
                    0.0
                }
            }
        };
        progress - jump
    }

    fn end(&self, start: &SonicState, now: &SonicState) -> Option<End> {
        if died(start, now) {
            Some(End::Died)
        } else if finished(start, now) {
            Some(End::Completed)
        } else {
            None
        }
    }

    fn feature_count(&self) -> usize {
        self.features.count()
    }

    fn features(&self, s: &SonicState, out: &mut [f32]) {
        let progress = if s.limit_right > 0 {
            f32::from(s.x) / f32::from(s.limit_right)
        } else {
            0.0
        };
        out[0] = progress.min(1.5);
        out[1] = f32::from(s.y) / FEATURE_Y_RANGE;
        out[2] = f32::from(s.x.wrapping_sub(s.camera_x) as i16) / FEATURE_SCREEN_WIDTH;
        out[3] = f32::from(s.speed_x) / FEATURE_SPEED_RANGE;
        out[4] = f32::from(s.speed_y) / FEATURE_SPEED_RANGE;
        out[5] = f32::from(s.rings.min(FEATURE_RINGS_CAP)) / f32::from(FEATURE_RINGS_CAP);
        if self.features != FeatureSet::Basic {
            let turn = f32::from(s.angle) * std::f32::consts::TAU / 256.0;
            out[6] = f32::from(s.inertia) / FEATURE_SPEED_RANGE;
            out[7] = turn.sin();
            out[8] = turn.cos();
            out[9] = f32::from(u8::from(s.status & STATUS_IN_AIR != 0));
            out[10] = f32::from(u8::from(s.status & STATUS_ROLLING != 0));
        }
        if matches!(
            self.features,
            FeatureSet::Full | FeatureSet::Objects | FeatureSet::Hazards
        ) {
            out[11] = f32::from(u8::from(s.joypad_held & BUTTONS_JUMP != 0));
        }
        let shown = match self.features {
            FeatureSet::Objects => DANGERS,
            FeatureSet::Hazards => HAZARDS,
            _ => 0,
        };
        for (i, danger) in s.dangers.iter().take(shown).enumerate() {
            let cell = &mut out[12 + 4 * i..16 + 4 * i];
            cell.copy_from_slice(&match danger {
                Some(d) => [
                    1.0,
                    f32::from(d.dx) / FEATURE_SCREEN_WIDTH,
                    f32::from(d.dy) / FEATURE_SCREEN_HEIGHT,
                    f32::from(u8::from(d.badnik)),
                ],
                None => [0.0; 4],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reward of one step with a fresh episode memory.
    fn reward(g: &Sonic1, prev: &SonicState, now: &SonicState) -> f32 {
        g.reward(&mut SonicMemory::default(), prev, now)
    }

    /// Sonic playing at `x`: routine Control, timer running.
    fn state(x: u16) -> SonicState {
        SonicState {
            x,
            lives: 3,
            zone: 0,
            act: 0,
            routine: 2,
            time_count: 1,
            ..SonicState::default()
        }
    }

    #[test]
    fn variables_are_read_where_the_sources_put_them() {
        let mut ram = vec![0u8; 0x10000];
        let put = |ram: &mut Vec<u8>, addr: u32, bytes: &[u8]| {
            let i = (addr & 0xFFFF) as usize;
            ram[i..i + bytes.len()].copy_from_slice(bytes);
        };
        put(&mut ram, addr::X, &[0x01, 0x2C]); // 300
        put(&mut ram, addr::Y, &[0x02, 0x00]); // 512
        put(&mut ram, addr::SPEED_X, &[0xFF, 0x00]); // -256
        put(&mut ram, addr::SPEED_Y, &[0x00, 0x80]); // 128
        put(&mut ram, addr::CAMERA_X, &[0x00, 0xC8]); // 200
        put(&mut ram, addr::LIMIT_RIGHT, &[0x25, 0x60]);
        put(&mut ram, addr::ZONE, &[2]);
        put(&mut ram, addr::ACT, &[1]);
        put(&mut ram, addr::LIVES, &[3]);
        put(&mut ram, addr::RINGS, &[0x00, 0x2A]); // 42
        put(&mut ram, addr::SCORE, &[0x00, 0x00, 0x27, 0x10]); // 10000
        put(&mut ram, addr::ROUTINE, &[6]);
        put(&mut ram, addr::TIME_COUNT, &[1]);
        put(&mut ram, addr::INERTIA, &[0xF8, 0x00]); // -2048
        put(&mut ram, addr::ANGLE, &[0x40]);
        put(&mut ram, addr::STATUS, &[0x06]);
        put(&mut ram, addr::JUMPING, &[1]);
        put(&mut ram, addr::JOYPAD_HELD, &[0x18]);
        let s = Sonic1::default().read(&ram);
        assert_eq!(
            s,
            SonicState {
                x: 300,
                y: 512,
                speed_x: -256,
                speed_y: 128,
                camera_x: 200,
                limit_right: 0x2560,
                zone: 2,
                act: 1,
                lives: 3,
                rings: 42,
                score: 10_000,
                routine: 6,
                time_count: 1,
                inertia: -2048,
                angle: 0x40,
                status: 0x06,
                jumping: 1,
                joypad_held: 0x18,
                dangers: [None; HAZARDS],
            }
        );
    }

    #[test]
    fn the_reward_follows_the_progress_to_the_right() {
        let g = Sonic1::default();
        assert!(
            (reward(&g, &state(100), &state(110)) - 0.10).abs() < 1e-6,
            "10 pixels right"
        );
        assert!(
            (reward(&g, &state(110), &state(100)) + 0.10).abs() < 1e-6,
            "left costs"
        );
        assert_eq!(reward(&g, &state(100), &state(100)), 0.0);
        let wrapped = reward(&g, &state(0xFFFE), &state(0x0002));
        assert!(
            (wrapped - 0.04).abs() < 1e-6,
            "a wrapping X is 4 pixels, not 65532"
        );
    }

    #[test]
    fn a_jump_costs_its_penalty_once() {
        let g = Sonic1 {
            jump_penalty: 0.5,
            ..Sonic1::default()
        };
        let ground = state(100);
        let jumping = SonicState {
            jumping: 1,
            x: 110,
            ..ground
        };
        assert!(
            (reward(&g, &ground, &jumping) - (0.10 - 0.5)).abs() < 1e-6,
            "the take-off step: progress minus the penalty"
        );
        let still_up = SonicState { x: 120, ..jumping };
        assert!(
            (reward(&g, &jumping, &still_up) - 0.10).abs() < 1e-6,
            "in the air: free"
        );
        let landed = SonicState {
            jumping: 0,
            ..still_up
        };
        assert_eq!(reward(&g, &still_up, &landed), 0.0, "landing: free");
        let free = Sonic1::default();
        assert!(
            (reward(&free, &ground, &jumping) - 0.10).abs() < 1e-6,
            "no penalty by default"
        );
        // Death and completion keep their own reward, jump or not.
        let dead = SonicState {
            lives: 2,
            ..jumping
        };
        assert_eq!(reward(&g, &ground, &dead), -10.0);
        let next_act = SonicState { act: 1, ..jumping };
        assert_eq!(reward(&g, &ground, &next_act), 50.0);
    }

    #[test]
    fn dying_and_finishing_have_their_own_reward_and_end() {
        let g = Sonic1::default();
        let start = state(100);
        let dead = SonicState {
            lives: 2,
            x: 5000,
            ..start
        };
        assert_eq!(
            reward(&g, &start, &dead),
            -10.0,
            "no credit for X on the way"
        );
        assert_eq!(g.end(&start, &dead), Some(End::Died));
        let next_act = SonicState {
            act: 1,
            x: 40,
            ..start
        };
        assert_eq!(reward(&g, &start, &next_act), 50.0);
        assert_eq!(g.end(&start, &next_act), Some(End::Completed));
        assert_eq!(g.end(&start, &state(900)), None, "running on");
        // An extra life is not a reason to stop.
        let extra = SonicState { lives: 4, ..start };
        assert_eq!(g.end(&start, &extra), None);
    }

    #[test]
    fn death_is_seen_at_the_fatal_hit() {
        let g = Sonic1::default();
        let start = state(100);
        let hit = SonicState {
            routine: 6,
            x: 765,
            ..start
        };
        assert_eq!(
            g.end(&start, &hit),
            Some(End::Died),
            "routine Death, lives not yet lost"
        );
        assert_eq!(
            reward(&g, &state(765), &hit),
            -10.0,
            "penalty at the same step"
        );
        let reset = SonicState { routine: 8, ..hit };
        assert_eq!(g.end(&start, &reset), Some(End::Died), "ResetLevel too");
        assert_eq!(reward(&g, &hit, &reset), 0.0, "the penalty is given once");
        let hurt = SonicState {
            routine: 4,
            ..start
        };
        assert_eq!(g.end(&start, &hurt), None, "hurt with rings is not a death");
        assert_eq!(reward(&g, &start, &hurt), 0.0);
    }

    #[test]
    fn the_signpost_ends_the_level_when_the_timer_stops() {
        let g = Sonic1::default();
        let start = state(9241);
        let signpost = SonicState {
            time_count: 0,
            x: 9600,
            ..start
        };
        assert_eq!(
            g.end(&start, &signpost),
            Some(End::Completed),
            "act not changed yet"
        );
        assert_eq!(
            reward(&g, &state(9590), &signpost),
            50.0,
            "bonus at the same step"
        );
        let tally = SonicState {
            x: 9767,
            ..signpost
        };
        // Then progress only: (9767 - 9600) pixels at 0.01.
        assert!((reward(&g, &signpost, &tally) - 1.67).abs() < 1e-5);
        // The timer also stops when Sonic dies: that is a death.
        let dying = SonicState {
            time_count: 0,
            routine: 6,
            ..start
        };
        assert_eq!(g.end(&start, &dying), Some(End::Died));
        assert_eq!(reward(&g, &start, &dying), -10.0);
        // A start state with the timer stopped never counts as finished.
        let stopped = SonicState {
            time_count: 0,
            ..start
        };
        assert_eq!(g.end(&stopped, &stopped), None);
    }

    #[test]
    fn best_mode_pays_only_for_new_ground() {
        let g = Sonic1 {
            reward_mode: RewardMode::Best,
            ..Sonic1::default()
        };
        let mut memory = SonicMemory::default();
        let mut step = |prev: u16, now: u16| g.reward(&mut memory, &state(prev), &state(now));
        assert!((step(100, 110) - 0.10).abs() < 1e-6, "10 new pixels");
        assert_eq!(step(110, 90), 0.0, "going back costs nothing");
        assert_eq!(step(90, 110), 0.0, "and coming back gains nothing");
        assert!(
            (step(110, 115) - 0.05).abs() < 1e-6,
            "only beyond the best (110)"
        );
        let wrapped = Sonic1 {
            reward_mode: RewardMode::Best,
            ..Sonic1::default()
        };
        let mut m = SonicMemory {
            best_x: Some(0xFFFE),
        };
        assert!(
            (wrapped.reward(&mut m, &state(0xFFFE), &state(2)) - 0.04).abs() < 1e-6,
            "wraps"
        );
        assert_eq!(g.progress(&state(4321)), 4321);
        // Death and completion are rewarded as in Delta mode.
        let start = state(100);
        assert_eq!(
            g.reward(
                &mut SonicMemory::default(),
                &start,
                &SonicState {
                    routine: 6,
                    ..start
                }
            ),
            -10.0
        );
    }

    #[test]
    fn features_are_about_unit_size() {
        let g = Sonic1::default();
        let s = SonicState {
            x: 4800,
            y: 1024,
            speed_x: 2048,
            speed_y: -2048,
            camera_x: 4700,
            limit_right: 9600,
            rings: 250,
            ..state(4800)
        };
        let mut out = [0.0f32; 6];
        g.features(&s, &mut out);
        let expected = [0.5, 0.5, 100.0 / 320.0, 0.5, -0.5, 1.0];
        for (got, want) in out.iter().zip(expected) {
            assert!((got - want).abs() < 1e-6, "{out:?} vs {expected:?}");
        }
        g.features(&SonicState::default(), &mut out);
        assert_eq!(out[0], 0.0, "no limit read yet: no division by zero");
    }

    #[test]
    fn dangers_are_the_nearest_badniks_and_harmful_objects_on_screen() {
        let mut ram = vec![0u8; 0x10000];
        let put = |ram: &mut Vec<u8>, addr: u32, bytes: &[u8]| {
            let i = (addr & 0xFFFF) as usize;
            ram[i..i + bytes.len()].copy_from_slice(bytes);
        };
        put(&mut ram, addr::X, &1000u16.to_be_bytes());
        put(&mut ram, addr::Y, &500u16.to_be_bytes());
        // (slot, x, y, obColType, rendered)
        for (slot, x, y, col, shown) in [
            (0u32, 1300u16, 500u16, 0x06u8, true), // badnik, 300 ahead
            (5, 950, 510, 0x84, true),             // spikes, 50 behind
            (7, 1010, 500, 0x47, true),            // ring: an item, not a danger
            (9, 1020, 500, 0x06, false),           // badnik off screen
            (11, 1005, 500, 0x00, true),           // no collision
            (95, 1200, 400, 0x0C, true),           // badnik, farther than the spikes
        ] {
            let base = addr::LEVEL_OBJECTS + slot * object::SIZE;
            put(&mut ram, base + object::X, &x.to_be_bytes());
            put(&mut ram, base + object::Y, &y.to_be_bytes());
            put(&mut ram, base + object::COL_TYPE, &[col]);
            put(
                &mut ram,
                base + object::RENDER,
                &[if shown { 0x84 } else { 0x04 }],
            );
        }
        let g = Sonic1 {
            features: FeatureSet::Objects,
            ..Sonic1::default()
        };
        let s = g.read(&ram);
        assert_eq!(
            s.dangers,
            [
                Some(Danger {
                    dx: -50,
                    dy: 10,
                    badnik: false
                }),
                Some(Danger {
                    dx: 200,
                    dy: -100,
                    badnik: true
                }),
                Some(Danger {
                    dx: 300,
                    dy: 0,
                    badnik: true
                }),
            ]
        );
        assert_eq!(g.feature_count(), 20);
        let mut out = [9.0f32; 20];
        g.features(&s, &mut out);
        assert_eq!(out[12..16], [1.0, -50.0 / 320.0, 10.0 / 224.0, 0.0]);
        assert_eq!(out[16..20], [1.0, 200.0 / 320.0, -100.0 / 224.0, 1.0]);
        let alone = SonicState {
            dangers: [None; HAZARDS],
            ..s
        };
        g.features(&alone, &mut out);
        assert_eq!(out[12..20], [0.0; 8], "no danger on screen");
    }

    #[test]
    fn hazards_add_spikes_and_a_third_danger() {
        let mut ram = vec![0u8; 0x10000];
        let put = |ram: &mut Vec<u8>, addr: u32, bytes: &[u8]| {
            let i = (addr & 0xFFFF) as usize;
            ram[i..i + bytes.len()].copy_from_slice(bytes);
        };
        put(&mut ram, addr::X, &1000u16.to_be_bytes());
        put(&mut ram, addr::Y, &500u16.to_be_bytes());
        // (slot, object id, x, y, obColType): spikes have no obColType.
        for (slot, id, x, y, col) in [
            (2u32, object::ID_SPIKES, 1040u16, 516u16, 0x00u8), // spikes, 40 ahead
            (3, 0x1F, 1100, 500, 0x06),                         // badnik, 100 ahead
            (4, object::ID_SPIKES, 1200, 516, 0x00),            // spikes, 200 ahead
            (6, 0x18, 1010, 500, 0x00),                         // a platform: harmless
        ] {
            let base = addr::LEVEL_OBJECTS + slot * object::SIZE;
            put(&mut ram, base + object::ID, &[id]);
            put(&mut ram, base + object::X, &x.to_be_bytes());
            put(&mut ram, base + object::Y, &y.to_be_bytes());
            put(&mut ram, base + object::COL_TYPE, &[col]);
            put(&mut ram, base + object::RENDER, &[0x84]);
        }
        let objects = Sonic1 {
            features: FeatureSet::Objects,
            ..Sonic1::default()
        };
        let badnik = Danger {
            dx: 100,
            dy: 0,
            badnik: true,
        };
        assert_eq!(
            objects.read(&ram).dangers,
            [Some(badnik), None, None],
            "Objects does not see spikes"
        );
        let hazards = Sonic1 {
            features: FeatureSet::Hazards,
            ..Sonic1::default()
        };
        let s = hazards.read(&ram);
        let spikes = |dx| Danger {
            dx,
            dy: 16,
            badnik: false,
        };
        assert_eq!(
            s.dangers,
            [Some(spikes(40)), Some(badnik), Some(spikes(200))]
        );
        assert_eq!(hazards.feature_count(), 24);
        let mut out = [9.0f32; 24];
        hazards.features(&s, &mut out);
        assert_eq!(out[12..16], [1.0, 40.0 / 320.0, 16.0 / 224.0, 0.0]);
        assert_eq!(out[16..20], [1.0, 100.0 / 320.0, 0.0, 1.0]);
        assert_eq!(out[20..24], [1.0, 200.0 / 320.0, 16.0 / 224.0, 0.0]);
        // Objects shows the first 2 of them only.
        let mut twenty = [9.0f32; 20];
        objects.features(&s, &mut twenty);
        assert_eq!(twenty[12..20], out[12..20]);
    }

    #[test]
    fn full_features_add_the_jump_button_held() {
        let g = Sonic1 {
            features: FeatureSet::Full,
            ..Sonic1::default()
        };
        assert_eq!(g.feature_count(), 12);
        let extended = Sonic1 {
            features: FeatureSet::Extended,
            ..Sonic1::default()
        };
        let base = SonicState {
            inertia: 2048,
            angle: 64,
            status: STATUS_IN_AIR,
            ..state(100)
        };
        let mut eleven = [0.0f32; 11];
        extended.features(&base, &mut eleven);
        let mut out = [9.0f32; 12];
        for (held, expected) in [
            (0x10u8, 1.0),
            (0x20, 1.0),
            (0x40, 1.0),
            (0x08, 0.0),
            (0, 0.0),
        ] {
            g.features(
                &SonicState {
                    joypad_held: held,
                    ..base
                },
                &mut out,
            );
            assert_eq!(out[11], expected, "held {held:#04x}");
            assert_eq!(out[..11], eleven, "the 11 of Extended first");
        }
    }

    #[test]
    fn extended_features_add_what_a_loop_needs() {
        let g = Sonic1 {
            features: FeatureSet::Extended,
            ..Sonic1::default()
        };
        assert_eq!(
            (g.feature_count(), Sonic1::default().feature_count()),
            (11, 6)
        );
        let mut out = [9.0f32; 11];
        let base = SonicState {
            inertia: 2048,
            status: STATUS_IN_AIR,
            ..state(100)
        };
        // Quarter turns: 0 flat, 64, 128 upside down, 192.
        for (angle, sin, cos) in [
            (0u8, 0.0, 1.0),
            (64, 1.0, 0.0),
            (128, 0.0, -1.0),
            (192, -1.0, 0.0),
        ] {
            g.features(&SonicState { angle, ..base }, &mut out);
            assert!(
                (out[7] - sin).abs() < 1e-6 && (out[8] - cos).abs() < 1e-6,
                "angle {angle}: {out:?}"
            );
        }
        assert_eq!(
            (out[6], out[9], out[10]),
            (0.5, 1.0, 0.0),
            "ground speed, in the air, not rolling"
        );
        g.features(
            &SonicState {
                status: STATUS_ROLLING,
                ..base
            },
            &mut out,
        );
        assert_eq!((out[9], out[10]), (0.0, 1.0));
        // The first six are the basic ones.
        let mut basic = [0.0f32; 6];
        Sonic1::default().features(&base, &mut basic);
        assert_eq!(&out[..6], &basic);
    }

    #[test]
    fn the_action_set_presses_what_the_names_say() {
        let a = Sonic1::ACTIONS;
        assert_eq!(a.len(), 7);
        assert_eq!(a[0], Buttons::NONE);
        assert!(a[2].contains(Buttons::RIGHT) && a[2].contains(Buttons::B));
        assert!(a[6].contains(Buttons::LEFT) && a[6].contains(Buttons::B));
        assert!(
            !a.iter().any(|b| b.contains(Buttons::START)),
            "never pauses the game"
        );
    }
}
