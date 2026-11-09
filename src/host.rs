//! Fenêtre, son, clavier et manette (SDL2) ; boucle du moteur sans CPU ; mode CPU émulé.

use crate::machine::Machine;
use anyhow::{anyhow, Result};
use md_core::{audio::OUTPUT_RATE, Buttons, Cartridge, Console, MdConfig, PadKind, Region};
use sdl2::{audio::AudioSpecDesired, event::Event, keyboard::Keycode, pixels::PixelFormatEnum};
use std::time::{Duration, Instant};

pub fn rom_path() -> String {
    std::env::var("SONIC_ROM").unwrap_or_else(|_| "/home/kevin/Projet_IA/Jeux_video/binaries_source/Sonic The Hedgehog (USA, Europe).md".to_string())
}

fn sdl<T, E: ToString>(r: std::result::Result<T, E>) -> Result<T> {
    r.map_err(|e| anyhow!(e.to_string()))
}

fn pad(keys: &std::collections::HashSet<Keycode>) -> Buttons {
    let map = [
        (Keycode::Up, Buttons::UP),
        (Keycode::Down, Buttons::DOWN),
        (Keycode::Left, Buttons::LEFT),
        (Keycode::Right, Buttons::RIGHT),
        (Keycode::W, Buttons::A),
        (Keycode::X, Buttons::B),
        (Keycode::C, Buttons::C),
        (Keycode::Return, Buttons::START),
    ];
    map.iter().filter(|(k, _)| keys.contains(k)).fold(Buttons::NONE, |a, (_, b)| a | *b)
}

/// Manettes SDL (GameController, avec repli sur l'API joystick brute pour les modèles inconnus de SDL).
struct Pads {
    gc_sub: sdl2::GameControllerSubsystem,
    js_sub: sdl2::JoystickSubsystem,
    controllers: Vec<sdl2::controller::GameController>,
    joysticks: Vec<sdl2::joystick::Joystick>,
}

impl Pads {
    fn new(ctx: &sdl2::Sdl) -> Result<Pads> {
        let mut p = Pads { gc_sub: sdl(ctx.game_controller())?, js_sub: sdl(ctx.joystick())?, controllers: Vec::new(), joysticks: Vec::new() };
        let n = p.js_sub.num_joysticks().unwrap_or(0);
        for i in 0..n { p.open(i); }
        Ok(p)
    }
    fn open(&mut self, idx: u32) {
        if self.gc_sub.is_game_controller(idx) {
            if let Ok(c) = self.gc_sub.open(idx) { eprintln!("manette : {}", c.name()); self.controllers.push(c); }
        } else if let Ok(j) = self.js_sub.open(idx) {
            eprintln!("manette (joystick brut) : {}", j.name());
            self.joysticks.push(j);
        }
    }
    fn event(&mut self, e: &Event) {
        match e {
            Event::ControllerDeviceAdded { which, .. } | Event::JoyDeviceAdded { which, .. } => {
                // Une manette reconnue apparaît sous les deux événements : on ne l'ouvre qu'une fois.
                if !self.controllers.iter().any(|c| c.instance_id() == *which as u32) && !self.joysticks.iter().any(|j| j.instance_id() == *which as u32) {
                    self.open(*which);
                }
            }
            Event::ControllerDeviceRemoved { which, .. } | Event::JoyDeviceRemoved { which, .. } => {
                self.controllers.retain(|c| c.instance_id() != *which as u32 && c.attached());
                self.joysticks.retain(|j| j.instance_id() != *which as u32 && j.attached());
            }
            _ => {}
        }
    }
    /// Boutons Mega Drive appuyés. A/X -> saut (A, B, C équivalents pour Sonic), Start/Menu -> Start.
    fn buttons(&self) -> Buttons {
        use sdl2::controller::{Axis, Button as SB};
        const DEAD: i16 = 9000;
        let mut b = Buttons::NONE;
        for c in &self.controllers {
            let (x, y) = (c.axis(Axis::LeftX), c.axis(Axis::LeftY));
            if c.button(SB::DPadLeft) || x < -DEAD { b = b | Buttons::LEFT; }
            if c.button(SB::DPadRight) || x > DEAD { b = b | Buttons::RIGHT; }
            if c.button(SB::DPadUp) || y < -DEAD { b = b | Buttons::UP; }
            if c.button(SB::DPadDown) || y > DEAD { b = b | Buttons::DOWN; }
            if c.button(SB::A) { b = b | Buttons::B; }
            if c.button(SB::B) || c.button(SB::RightShoulder) { b = b | Buttons::C; }
            if c.button(SB::X) || c.button(SB::Y) || c.button(SB::LeftShoulder) { b = b | Buttons::A; }
            if c.button(SB::Start) || c.button(SB::Back) { b = b | Buttons::START; }
        }
        for j in &self.joysticks {
            let (x, y) = (j.axis(0).unwrap_or(0), j.axis(1).unwrap_or(0));
            let hat = j.hat(0).ok();
            use sdl2::joystick::HatState as H;
            if x < -DEAD || matches!(hat, Some(H::Left | H::LeftUp | H::LeftDown)) { b = b | Buttons::LEFT; }
            if x > DEAD || matches!(hat, Some(H::Right | H::RightUp | H::RightDown)) { b = b | Buttons::RIGHT; }
            if y < -DEAD || matches!(hat, Some(H::Up | H::LeftUp | H::RightUp)) { b = b | Buttons::UP; }
            if y > DEAD || matches!(hat, Some(H::Down | H::LeftDown | H::RightDown)) { b = b | Buttons::DOWN; }
            if j.button(0).unwrap_or(false) { b = b | Buttons::B; }
            if j.button(1).unwrap_or(false) { b = b | Buttons::C; }
            if j.button(2).unwrap_or(false) || j.button(3).unwrap_or(false) { b = b | Buttons::A; }
            if (7..=9).any(|k| j.button(k).unwrap_or(false)) { b = b | Buttons::START; }
        }
        b
    }
}


struct Gfx {
    canvas: sdl2::render::Canvas<sdl2::video::Window>,
    tex: sdl2::render::Texture<'static>,
    tc: &'static sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    events: sdl2::EventPump,
    queue: sdl2::audio::AudioQueue<i16>,
    keys: std::collections::HashSet<Keycode>,
    pads: Pads,
    next: Instant,
    _ctx: sdl2::Sdl,
    /// Facteur d'agrandissement (SONIC_SCALE, 1 = image d'origine) et tampon de l'image agrandie.
    scale: usize,
    hd: Vec<u32>,
    /// SONIC_PERF=1 : temps cumulés (ms) émulation, image agrandie, affichage, attente ; nombre d'images ; fin de la dernière image.
    perf: Option<([f64; 4], u32, Instant)>,
}


struct Host { seen: std::collections::HashSet<crate::hd::TileKey>, pack: Option<crate::hd::Pack>, frames: u64, limit: Option<u64>, dump: Option<String>, gfx: Option<Gfx> }

thread_local! { static PENDING_TRAP: std::cell::RefCell<Option<sonic1_rt::Trap>> = const { std::cell::RefCell::new(None) }; }
thread_local! { static HOST: std::cell::RefCell<Option<Host>> = const { std::cell::RefCell::new(None) }; }

fn write_ppm(path: &str, c: &Console) {
    let (w, h) = (c.screen_width(), c.screen_height());
    let rgba = c.frame_rgba();
    let mut out = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in rgba.chunks(4) { out.extend_from_slice(&px[..3]); }
    let _ = std::fs::write(path, out);
}


/// Appelé à chaque image terminée par le moteur sans CPU.
fn host_frame(c: &mut Console) -> bool {
    HOST.with(|h| {
        let mut h = h.borrow_mut();
        let h = h.as_mut().unwrap();
        h.frames += 1;
        if let (Ok(dir), true) = (std::env::var("EXPORT_TILES"), h.frames % 30 == 0) { crate::pack::export_seen(c, &dir, &mut h.seen); }
        if h.frames % 60 == 0 { if let (Some(pk), true) = (h.pack.as_ref(), std::env::var_os("HD_CHECK").is_some()) { crate::hd::check_pack(c, pk, h.frames); } }
        if h.frames == 200 { PENDING_TRAP.with(|p| if let Some(t) = p.borrow_mut().take() { let _ = sonic1_rt::TRAP.set(t); }); }
        if let Some(n) = h.limit {
            // Mode sans fenêtre : script d'entrées (Start sur l'écran titre, puis course vers la droite avec des sauts).
            let f = h.frames;
            let btn = if (730..740).contains(&f) { Buttons::START }
                else if f > 1200 { let mut b = Buttons::RIGHT; if f % 90 < 25 { b = b | Buttons::C; } b }
                else { Buttons::NONE };
            c.set_buttons(0, btn);
            // NOCPU_POKE="image:adresse:valeur,..." : écrit des octets en RAM (ex. 900:F600:10 = mode « bonus » à l'image 900).
            if let Ok(spec) = std::env::var("NOCPU_POKE") {
                for item in spec.split(',') {
                    let v: Vec<&str> = item.split(':').collect();
                    if v.len() == 3 && v[0].parse::<u64>().ok() == Some(f) {
                        if let (Ok(a), Ok(x)) = (u32::from_str_radix(v[1], 16), u8::from_str_radix(v[2], 16)) { c.bus_mut().write_work_ram(a, x); }
                    }
                }
            }
            // HASH_EVERY=1 : empreinte de l'image et de la RAM à chaque image (comparer données ROM / fichier d'assets).
            if std::env::var_os("HASH_EVERY").is_some() {
                let ram_hash = c.bus().work_ram().iter().fold(0xcbf29ce484222325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100000001b3));
                eprintln!("H {f} {:016x} {ram_hash:016x}", c.frame_hash());
            }
            // TRACE_RAM=adr:len,... (hexa) : affiche ces octets de la RAM à chaque image (comparaison avec REF_CPU).
            if let Ok(spec) = std::env::var("TRACE_RAM") {
                let ram = c.bus().work_ram();
                let mut line = format!("T {f}");
                for it in spec.split(',') { if let Some((a, l)) = it.split_once(':') { if let (Ok(a), Ok(l)) = (usize::from_str_radix(a, 16), usize::from_str_radix(l, 16)) { line += &format!(" [{}]", ram[a & 0xFFFF..(a & 0xFFFF) + l].iter().map(|b| format!("{b:02x}")).collect::<String>()); } } }
                eprintln!("{line}");
            }
            if h.frames % 60 == 0 && std::env::var_os("PROV_CHECK").is_some() { prov_check(c, h.frames); }
            if h.frames % 60 == 0 { if let Some(sc) = std::env::var("HD_CHECK").ok().and_then(|v| v.parse().ok()) { crate::hd::check(c, sc, h.frames); if std::env::var_os("HD_ORIENT").is_some() { crate::hd::check_orientation(c, sc, h.frames); } } }
            if h.frames % 60 == 0 {
                let ram = c.bus().work_ram();
                eprintln!("image {} : empreinte {:016x} mode {:#04x} FE04={}", h.frames, c.frame_hash(), ram[0xF600], u16::from_be_bytes([ram[0xFE04], ram[0xFE05]]));
            }
            if let (Ok(dir), true) = (std::env::var("EXPORT_DIR"), h.frames % std::env::var("EXPORT_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(600) == 0) { export_snapshot(c, &dir, h.frames); }
            if h.frames >= n {
                if let Some(p) = &h.dump {
                    // Avec SONIC_SCALE ou SONIC_PACK : sauvegarde l'image agrandie (PNG si le nom finit par .png) au lieu de l'originale.
                    let scale = std::env::var("SONIC_SCALE").ok().and_then(|v| v.parse().ok()).or(h.pack.as_ref().map(|k| k.scale)).unwrap_or(if std::env::var_os("SONIC_FILTER").is_some() { 3 } else { 1 });
                    if scale > 1 {
                        let mut buf = Vec::new();
                        let (ow, oh) = crate::hd::render(c, scale, h.pack.as_ref(), &mut buf);
                        let rgb: Vec<u8> = buf.iter().flat_map(|p| [(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8]).collect();
                        write_png(p, ow, oh, &rgb);
                    } else { write_ppm(p, c); }
                }
                return false;
            }
        }
        if let Some(g) = h.gfx.as_mut() {
            for e in g.events.poll_iter() {
                g.pads.event(&e);
                match e {
                    Event::Quit { .. } | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => return false,
                    Event::KeyDown { keycode: Some(k), .. } => { g.keys.insert(k); }
                    Event::KeyUp { keycode: Some(k), .. } => { g.keys.remove(&k); }
                    _ => {}
                }
            }
            c.set_buttons(0, pad(&g.keys) | g.pads.buttons());
            let t_start = Instant::now();
            let (mut w, mut hh) = (c.screen_width() as u32, c.screen_height() as u32);
            // Image agrandie : pixels réordonnés en place pour que la mémoire soit R,V,B,A (octets) sans copie ni allocation.
            let converted: Vec<u8>;
            let rgba: &[u8] = if g.scale > 1 {
                let (ow, oh) = crate::hd::render(c, g.scale, h.pack.as_ref(), &mut g.hd);
                (w, hh) = (ow as u32, oh as u32);
                if cfg!(target_endian = "little") {
                    for p in g.hd.iter_mut() { *p = p.swap_bytes(); }
                    unsafe { std::slice::from_raw_parts(g.hd.as_ptr() as *const u8, g.hd.len() * 4) }
                } else {
                    converted = g.hd.iter().flat_map(|p| p.to_be_bytes()).collect();
                    &converted
                }
            } else { converted = c.frame_rgba(); &converted };
            let t_hd = Instant::now();
            if g.tex.query().width != w || g.tex.query().height != hh {
                g.tex = g.tc.create_texture_streaming(PixelFormatEnum::ABGR8888, w, hh).unwrap();
            }
            let _ = g.tex.update(None, rgba, (w * 4) as usize);
            g.canvas.clear();
            let _ = g.canvas.copy(&g.tex, None, None);
            g.canvas.present();
            let _ = g.queue.queue_audio(c.audio_samples());
            if g.queue.size() > 44_100 * 4 / 5 { g.queue.clear(); }
            let t_shown = Instant::now();
            g.next += Duration::from_micros(16_639);
            let now = Instant::now();
            if g.next > now { std::thread::sleep(g.next - now); } else { g.next = now; }
            if let Some((acc, n, last)) = g.perf.as_mut() {
                let ms = |a: Instant, b: Instant| b.duration_since(a).as_secs_f64() * 1000.0;
                acc[0] += ms(*last, t_start); acc[1] += ms(t_start, t_hd); acc[2] += ms(t_hd, t_shown); acc[3] += ms(t_shown, Instant::now());
                *n += 1; *last = Instant::now();
                if *n == 120 {
                    eprintln!("perf (moyenne sur 120 images, budget 16,6 ms) : émulation {:.1} ms, image agrandie {:.1} ms, affichage {:.1} ms, attente {:.1} ms", acc[0] / 120.0, acc[1] / 120.0, acc[2] / 120.0, acc[3] / 120.0);
                    *acc = [0.0; 4]; *n = 0;
                }
            }
        }
        true
    })
}

/// Fichier de données du jeu : `SONIC_ASSETS`, sinon `sonic1.assets` dans le dossier courant, sinon la ROM.
pub fn resolve_data_path() -> String {
    if let Ok(p) = std::env::var("SONIC_ASSETS") { return p; }
    if std::path::Path::new("sonic1.assets").exists() { return "sonic1.assets".into(); }
    rom_path()
}

/// Moteur sans CPU 68000 : fenêtre (`headless = None`) ou `N` images sans fenêtre avec script d'entrées.
pub fn run_nocpu(headless: Option<(u64, Option<String>)>) -> Result<()> {
    if let Ok(out) = std::env::var("TRAP_CODE") {
        // Espion : relève les lectures d'octets de code (en tant que données) dans le fichier `out` (adresses hexa).
        let rom = std::fs::read(rom_path())?;
        let mut map = vec![false; rom.len()];
        for a in crate::devtools::reliable_code_bytes(&rom) { if a >= 0x200 { map[a as usize] = true; } }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(out)?;
        PENDING_TRAP.with(|p| *p.borrow_mut() = Some(sonic1_rt::Trap { map, seen: Default::default(), file: std::sync::Mutex::new(file) }));
    }
    let path = resolve_data_path();
    let pack_loaded = match std::env::var("SONIC_PACK") { Ok(d) => Some(crate::pack::load(&d)?), Err(_) => None };
    eprintln!("données du jeu : {path}");
    let mut con = Console::new(Cartridge::from_file(&path)?, MdConfig { region: Region::Usa, tmss: false });
    con.connect_pad(0, Some(PadKind::ThreeButton));
    if std::env::var_os("PROV_CHECK").is_some() || std::env::var_os("HD_CHECK").is_some() || std::env::var_os("SONIC_SCALE").is_some() || std::env::var_os("SONIC_PACK").is_some() || std::env::var_os("SONIC_FILTER").is_some() || std::env::var_os("EXPORT_TILES").is_some() { con.bus_mut().vdp_mut().set_provenance(true); }
    let gfx = if headless.is_none() {
        let ctx = sdl(sdl2::init())?;
        let video = sdl(ctx.video())?;
        let window = sdl(video.window("Sonic 1", 960, 672).position_centered().resizable().build())?;
        let canvas = sdl(window.into_canvas().present_vsync().build())?;
        let tc: &'static _ = Box::leak(Box::new(canvas.texture_creator()));
        let tex = sdl(tc.create_texture_streaming(PixelFormatEnum::ABGR8888, 320, 224))?;
        let queue = sdl(sdl(ctx.audio())?.open_queue::<i16, _>(None, &AudioSpecDesired { freq: Some(OUTPUT_RATE as i32), channels: Some(2), samples: Some(1024) }))?;
        queue.resume();
        let events = sdl(ctx.event_pump())?;
        let pads = Pads::new(&ctx)?;
        Some(Gfx { canvas, tex, tc, events, queue, keys: Default::default(), pads, next: Instant::now(), _ctx: ctx, scale: std::env::var("SONIC_SCALE").ok().and_then(|v| v.parse().ok()).filter(|&n| (1..=6).contains(&n)).unwrap_or(pack_loaded.as_ref().map_or(if std::env::var_os("SONIC_FILTER").is_some() { 3 } else { 1 }, |p| p.scale)), hd: Vec::new(), perf: std::env::var_os("SONIC_PERF").map(|_| ([0.0; 4], 0, Instant::now())) })
    } else { None };
    let (limit, dump) = match &headless { Some((n, d)) => (Some(*n), d.clone()), None => (None, None) };
    HOST.with(|h| *h.borrow_mut() = Some(Host { seen: Default::default(), pack: pack_loaded, frames: 0, limit, dump, gfx }));
    // REF_CPU=1 : référence = vrai processeur 68000 émulé (avec la ROM d'origine, pas le fichier d'assets) pour comparer au code recompilé.
    if std::env::var_os("REF_CPU").is_some() {
        loop { con.run_frame(); if !host_frame(&mut con) { return Ok(()); } }
    }
    let mut m = Machine::standalone(&mut con, host_frame);
    m.a[7] = m.rd32(0);
    m.sr = 0x2700;
    let entry = m.rd32(4);
    let g = crate::generated::lookup(entry).ok_or_else(|| anyhow!("point d'entrée {entry:#x} non traduit"))?;
    let exit = g(&mut m);
    eprintln!("sortie du code généré en {exit:#x} : code non traduit à cet endroit");
    eprintln!("d={:x?}\na={:x?} sr={:#x}", m.d, m.a, m.sr);
    Err(anyhow!("arrêt sur du code non traduit"))
}

// ---------------------------------------------------------------------------------------------
// Export lisible (EXPORT_DIR) : écran, planche de tuiles de la VRAM, palettes, RAM de travail.
// ---------------------------------------------------------------------------------------------

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data { c ^= b as u32; for _ in 0..8 { c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 }; } }
    !c
}

/// PNG RGB 8 bits non compressé (blocs « stored »), sans dépendance.
pub fn write_png(path: &str, w: usize, h: usize, rgb: &[u8]) {
    let mut raw = Vec::with_capacity((w * 3 + 1) * h);
    for y in 0..h { raw.push(0); raw.extend_from_slice(&rgb[y * w * 3..(y + 1) * w * 3]); }
    let mut z = vec![0x78, 0x01];
    let chunks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, ch) in chunks.iter().enumerate() {
        z.push((i + 1 == chunks.len()) as u8);
        z.extend_from_slice(&(ch.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(ch.len() as u16)).to_le_bytes());
        z.extend_from_slice(ch);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8; 4], data: &[u8]| {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec(); body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    };
    let mut ihdr = (w as u32).to_be_bytes().to_vec();
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &z);
    chunk(b"IEND", &[]);
    let _ = std::fs::write(path, out);
}

fn export_snapshot(c: &Console, dir: &str, frame: u64) {
    let _ = std::fs::create_dir_all(dir);
    let base = format!("{dir}/f{frame:05}");
    let (w, h) = (c.screen_width(), c.screen_height());
    let rgba = c.frame_rgba();
    let rgb: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
    write_png(&format!("{base}_ecran.png"), w, h, &rgb);
    let v = c.bus().vdp();
    let cram = v.cram();
    // Palette : 4 lignes de 16 couleurs, 9 bits BGR -> RGB 8 bits ; fichier texte + image.
    let col = |i: usize| { let x = cram[i] as u32; [(((x >> 1) & 7) * 36) as u8, (((x >> 5) & 7) * 36) as u8, (((x >> 9) & 7) * 36) as u8] };
    let mut txt = String::new();
    for line in 0..4 { txt += &format!("ligne {line} : {}\n", (0..16).map(|i| { let k = col(line * 16 + i); format!("#{:02x}{:02x}{:02x}", k[0], k[1], k[2]) }).collect::<Vec<_>>().join(" ")); }
    let _ = std::fs::write(format!("{base}_palette.txt"), txt);
    // Planche de tuiles : 2048 tuiles 8x8 en 4 bits, 64 par ligne, une planche par ligne de palette.
    let vram = v.vram();
    for pal in 0..4 {
        let (tw, th) = (64 * 8, 32 * 8);
        let mut img = vec![0u8; tw * th * 3];
        for t in 0..2048usize {
            for y in 0..8 { for x in 0..8 {
                let byte = vram[t * 32 + y * 4 + x / 2];
                let ix = if x % 2 == 0 { byte >> 4 } else { byte & 15 } as usize;
                let k = if ix == 0 { [24, 24, 24] } else { col(pal * 16 + ix) };
                let (px, py) = ((t % 64) * 8 + x, (t / 64) * 8 + y);
                img[(py * tw + px) * 3..][..3].copy_from_slice(&k);
            } }
        }
        write_png(&format!("{base}_tuiles_pal{pal}.png"), tw, th, &img);
    }
    let _ = std::fs::write(format!("{base}_vram.bin"), vram);
    let _ = std::fs::write(format!("{base}_ram.bin"), c.bus().work_ram());
}

/// Contrôle de l'enregistrement d'origine des pixels : reconstruit chaque pixel (couleur de la tuile dans la VRAM, couleur
/// dans la CRAM, intensité) et le compare à l'image réellement produite. Affiche le nombre de pixels différents.
fn prov_check(c: &Console, frame: u64) {
    let v = c.bus().vdp();
    let (w, h) = (c.screen_width(), c.screen_height());
    let (vram, cram) = v.provenance_snapshot();
    let (prov, fb) = (v.provenance(), v.framebuffer());
    let level = |x: u32, mode: u32| -> u32 { let l = match mode { 1 => x, 2 => x + 7, _ => 2 * x }; (l * 255 + 7) / 14 };
    let (mut bad_index, mut bad_rgba, mut sprites, mut total) = (0u32, 0u32, 0u32, 0u32);
    let (mut ymin, mut ymax) = (999usize, 0usize);
    for y in 0..h { for x in 0..w {
        let p = prov[y * 320 + x];
        let (tile, col, row) = ((p & 0x7FF) as usize, ((p >> 11) & 7) as usize, ((p >> 14) & 7) as usize);
        let (hf, vf) = (p & (1 << 17) != 0, p & (1 << 18) != 0);
        let (idx, inten, layer) = (((p >> 19) & 0x3F) as usize, (p >> 25) & 3, (p >> 27) & 3);
        total += 1; if layer == 3 { sprites += 1; }
        if layer != 0 {
            let (tx, ty) = (if hf { 7 - col } else { col }, if vf { 7 - row } else { row });
            let byte = vram[(tile * 32 + ty * 4 + tx / 2) & 0xFFFF];
            let color = if tx % 2 == 0 { byte >> 4 } else { byte & 15 } as usize;
            if color != idx & 15 { bad_index += 1; }
        }
        let word = cram[idx & 63] as u32;
        let rgba = (level((word >> 1) & 7, inten) << 24) | (level((word >> 5) & 7, inten) << 16) | (level((word >> 9) & 7, inten) << 8) | 0xFF;
        if rgba != fb[y * 320 + x] { bad_rgba += 1; ymin = ymin.min(y); ymax = ymax.max(y); if std::env::var_os("PROV_WHY").is_some() && frame == 1440 { eprintln!("  pixel ({x},{y}) couche {layer} couleur {idx} : reconstruit {rgba:08x} vs image {:08x}", fb[y * 320 + x]); } }
    } }
    eprintln!("origine image {frame} : {total} pixels dont {sprites} de sprites ; {bad_index} tuiles incohérentes, {bad_rgba} couleurs différentes{}", if bad_rgba > 0 { format!(" (lignes {ymin} à {ymax})") } else { String::new() });
}
