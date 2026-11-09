//! Rendu en haute résolution : l'image finale de `md-core` est refaite à `scale` fois la taille, pixel par pixel, grâce à
//! l'origine enregistrée de chaque pixel (tuile, position dans la tuile, retournements, palette, couche).
//! Un pixel dont la tuile a une version haute résolution dans le `Pack` est échantillonné dans cette version ;
//! sinon il est simplement agrandi (bloc `scale × scale` de la couleur d'origine).

use md_core::Console;
use std::collections::HashMap;

/// Clé d'une tuile : contenu des 32 octets de la tuile et ligne de palette (0-3).
pub type TileKey = (u64, u8);

/// Une tuile de remplacement, de `8*scale × 8*scale` pixels, orientation d'origine : pour chaque pixel, la différence de couleur
/// (rouge, vert, bleu) entre la version améliorée et l'original agrandi sans amélioration. À l'écran, couleur affichée =
/// couleur actuelle + différence : l'eau, les fondus et les palettes animées teintent donc aussi la version améliorée.
pub struct PackTile { pub delta: Vec<[i16; 3]> }

impl PackTile {
    /// `hd` = version améliorée, `reference` = original agrandi, tous deux en RGBA 0xRRGGBBAA.
    pub fn new(hd: &[u32], reference: &[u32]) -> Self {
        let ch = |p: u32, sh: u32| ((p >> sh) & 255) as i16;
        PackTile { delta: hd.iter().zip(reference).map(|(&h, &r)| [ch(h, 24) - ch(r, 24), ch(h, 16) - ch(r, 16), ch(h, 8) - ch(r, 8)]).collect() }
    }
}

#[derive(Default)]
pub struct Pack {
    pub scale: usize,
    pub tiles: HashMap<TileKey, PackTile>,
}

pub fn tile_hash(tile: &[u8]) -> u64 {
    tile.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3))
}

/// Refait l'image à `scale` × ; retourne (largeur, hauteur) et remplit `out` (RGBA 0xRRGGBBAA, ligne par ligne).
pub fn render(c: &Console, scale: usize, pack: Option<&Pack>, out: &mut Vec<u32>) -> (usize, usize) {
    let v = c.bus().vdp();
    let (w, h) = (c.screen_width(), c.screen_height());
    let (fb, prov) = (v.framebuffer(), v.provenance());
    let (vram, _) = v.provenance_snapshot();
    let (ow, oh) = (w * scale, h * scale);
    if scale == 3 && pack.is_none() && scale3x_on() { scale3x(fb, w, h, out); return (ow, oh); }
    out.clear();
    out.resize(ow * oh, 0);
    // Une recherche dans le lot par (tuile, palette) et par image : on remplit d'abord le cache (séquentiel, 71 000 pixels),
    // puis les lignes sont calculées en parallèle (chacune est indépendante).
    let active = pack.filter(|pk| pk.scale == scale && !vram.is_empty());
    let mut found: Vec<Option<&PackTile>> = vec![None; if active.is_some() { 2048 * 4 } else { 0 }];
    if let Some(pk) = active {
        let mut state = vec![false; found.len()];
        for y in 0..h { for x in 0..w {
            let p = prov[y * 320 + x];
            if (p >> 27) & 3 == 0 { continue; }
            let slot = (p & 0x7FF) as usize * 4 + ((p >> 23) & 3) as usize;
            if !state[slot] {
                state[slot] = true;
                let t = slot / 4;
                found[slot] = pk.tiles.get(&(tile_hash(&vram[t * 32..t * 32 + 32]), (slot % 4) as u8));
            }
        } }
    }
    let found = &found;
    let rows = |y0: usize, band: &mut [u32]| {
        for (ry, block) in band.chunks_mut(scale * ow).enumerate() {
            let y = y0 + ry;
            for x in 0..w {
                let (color, p) = (fb[y * 320 + x], prov[y * 320 + x]);
                let tile = if active.is_some() && (p >> 27) & 3 != 0 { found[(p & 0x7FF) as usize * 4 + ((p >> 23) & 3) as usize] } else { None };
                match tile {
                    None => for sy in 0..scale { block[sy * ow + x * scale..][..scale].fill(color); },
                    Some(t) => {
                        let (col, rw) = (((p >> 11) & 7) as usize, ((p >> 14) & 7) as usize);
                        let (hf, vf) = (p & (1 << 17) != 0, p & (1 << 18) != 0);
                        let (r, g, b) = (((color >> 24) & 255) as i16, ((color >> 16) & 255) as i16, ((color >> 8) & 255) as i16);
                        for sy in 0..scale {
                            let ty = if vf { 7 - rw } else { rw } * scale + if vf { scale - 1 - sy } else { sy };
                            let row = &mut block[sy * ow + x * scale..][..scale];
                            for (sx, px) in row.iter_mut().enumerate() {
                                let tx = if hf { 7 - col } else { col } * scale + if hf { scale - 1 - sx } else { sx };
                                let d = t.delta[ty * 8 * scale + tx];
                                *px = (((r + d[0]).clamp(0, 255) as u32) << 24) | (((g + d[1]).clamp(0, 255) as u32) << 16) | (((b + d[2]).clamp(0, 255) as u32) << 8) | 0xFF;
                            }
                        }
                    }
                }
            }
        }
    };
    let threads = if active.is_some() { std::thread::available_parallelism().map_or(1, |n| n.get()).min(4) } else { 1 };
    let band_rows = h.div_ceil(threads);
    std::thread::scope(|sc| {
        for (i, band) in out.chunks_mut(band_rows * scale * ow).enumerate() {
            let rows = &rows;
            if threads == 1 { rows(i * band_rows, band); } else { sc.spawn(move || rows(i * band_rows, band)); }
        }
    });
    (ow, oh)
}

/// Lot « identité » (pour les tests) : chaque tuile de la VRAM, dans chacune des 4 lignes de palette, en agrandissement par
/// bloc avec les couleurs de la CRAM à intensité normale. Doit redonner exactement l'image agrandie.
pub fn identity_pack(c: &Console, scale: usize) -> Pack {
    let (vram, cram) = c.bus().vdp().provenance_snapshot();
    let lvl = |x: u32| (2 * x * 255 + 7) / 14;
    let rgba = |i: usize| { let wd = cram[i] as u32; (lvl((wd >> 1) & 7) << 24) | (lvl((wd >> 5) & 7) << 16) | (lvl((wd >> 9) & 7) << 8) | 0xFF };
    let mut pk = Pack { scale, tiles: HashMap::new() };
    for tile in 0..2048usize {
        let data = &vram[tile * 32..tile * 32 + 32];
        let hash = tile_hash(data);
        for pal in 0..4u8 {
            let mut img = vec![0u32; 64 * scale * scale];
            for ty in 0..8 { for tx in 0..8 {
                let byte = data[ty * 4 + tx / 2];
                let ix = if tx % 2 == 0 { byte >> 4 } else { byte & 15 } as usize;
                let color = rgba(pal as usize * 16 + ix);
                for sy in 0..scale { for sx in 0..scale { img[(ty * scale + sy) * 8 * scale + tx * scale + sx] = color; } }
            } }
            pk.tiles.entry((hash, pal)).or_insert(PackTile { delta: vec![[0; 3]; img.len()] });
        }
    }
    pk
}

/// Contrôle : image à `scale` × sans lot = chaque pixel d'origine agrandi ; avec le lot identité = la même image.
pub fn check(c: &Console, scale: usize, frame: u64) {
    let (mut plain, mut viapack) = (Vec::new(), Vec::new());
    let t0 = std::time::Instant::now();
    let (w, h) = render(c, scale, None, &mut plain);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    render(c, scale, Some(&identity_pack(c, scale)), &mut viapack);
    let fb = c.bus().vdp().framebuffer();
    let down = (0..h / scale).flat_map(|y| (0..w / scale).map(move |x| (x, y))).filter(|&(x, y)| plain[y * scale * w + x * scale] != fb[y * 320 + x]).count();
    let diff = plain.iter().zip(&viapack).filter(|(a, b)| a != b).count();
    eprintln!("hd image {frame} : {w}x{h}, réduit≠original {down}, lot identité≠agrandi {diff}, calcul {ms:.2} ms");
}

/// Contrôle avec un lot chargé depuis un dossier : image avec le lot comparée à l'image agrandie sans lot.
/// Affiche le nombre de pixels différents et la part des pixels de tuile pris dans le lot.
pub fn check_pack(c: &Console, pack: &Pack, frame: u64) {
    let (mut plain, mut with) = (Vec::new(), Vec::new());
    let scale = pack.scale;
    let (w, h) = render(c, scale, None, &mut plain);
    let t0 = std::time::Instant::now();
    render(c, scale, Some(pack), &mut with);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let t1 = std::time::Instant::now();
    let bytes: Vec<u8> = with.iter().flat_map(|p| p.to_be_bytes()).collect();
    let ms_conv = t1.elapsed().as_secs_f64() * 1000.0;
    let _ = bytes.len();
    let diff = plain.iter().zip(&with).filter(|(a, b)| a != b).count();
    let prov = c.bus().vdp().provenance();
    let (vram, _) = c.bus().vdp().provenance_snapshot();
    let (mut tiles, mut found) = (0u32, 0u32);
    for y in 0..h / scale { for x in 0..w / scale {
        let p = prov[y * 320 + x];
        if (p >> 27) & 3 != 0 && !vram.is_empty() {
            tiles += 1;
            let t = (p & 0x7FF) as usize;
            if pack.tiles.contains_key(&(tile_hash(&vram[t * 32..t * 32 + 32]), ((p >> 23) & 3) as u8)) { found += 1; }
        }
    } }
    eprintln!("lot image {frame} : pixels de tuiles couverts {found}/{tiles}, pixels agrandis différents du simple agrandissement {diff} ; calcul avec lot {ms:.1} ms, conversion en octets {ms_conv:.1} ms");
}

/// Contrôle d'orientation : dans chaque pixel de tuile, on marque (rouge +40) le sous-pixel en haut à gauche de la tuile
/// d'origine. À l'écran, ce marqueur doit tomber au sous-pixel (colonne, ligne) = (retourné H ? scale-1 : 0, retourné V ? scale-1 : 0).
pub fn check_orientation(c: &Console, scale: usize, frame: u64) {
    let mut pk = identity_pack(c, scale);
    for t in pk.tiles.values_mut() {
        for i in 0..t.delta.len() {
            let (tx, ty) = (i % (8 * scale), i / (8 * scale));
            if tx % scale == 0 && ty % scale == 0 { t.delta[i][0] += 40; }
        }
    }
    let (mut plain, mut marked) = (Vec::new(), Vec::new());
    let (w, h) = render(c, scale, None, &mut plain);
    render(c, scale, Some(&pk), &mut marked);
    let prov = c.bus().vdp().provenance();
    let (mut tested, mut bad, mut fh, mut fv) = (0u32, 0u32, 0u32, 0u32);
    for y in 0..h / scale { for x in 0..w / scale {
        let p = prov[y * 320 + x];
        if (p >> 27) & 3 == 0 || (plain[y * scale * w + x * scale] >> 24) > 215 { continue; }
        let (hf, vf) = (p & (1 << 17) != 0, p & (1 << 18) != 0);
        let want = (if hf { scale - 1 } else { 0 }, if vf { scale - 1 } else { 0 });
        let changed: Vec<(usize, usize)> = (0..scale * scale).map(|i| (i % scale, i / scale)).filter(|&(sx, sy)| plain[(y * scale + sy) * w + x * scale + sx] != marked[(y * scale + sy) * w + x * scale + sx]).collect();
        tested += 1; fh += hf as u32; fv += vf as u32;
        if changed != vec![want] { bad += 1; }
    } }
    eprintln!("orientation image {frame} : {tested} pixels testés, {bad} mal orientés (dont {fh} retournés H, {fv} retournés V)");
}

/// SONIC_FILTER=scale3x : filtre de pixel-art (Scale3x / AdvMAME3x) sur l'image entière, sans lot de tuiles (facteur 3 uniquement).
fn scale3x_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("SONIC_FILTER").map_or(false, |v| v.eq_ignore_ascii_case("scale3x")))
}

/// Scale3x : chaque pixel E devient 3×3 pixels ; un coin prend la couleur d'un voisin seulement quand deux voisins
/// côte à côte sont égaux et que les deux autres sont différents (c'est ce qui adoucit les diagonales sans flouter).
pub fn scale3x(fb: &[u32], w: usize, h: usize, out: &mut Vec<u32>) {
    let ow = w * 3;
    out.clear();
    out.resize(ow * h * 3, 0);
    let at = |x: isize, y: isize| fb[y.clamp(0, h as isize - 1) as usize * 320 + x.clamp(0, w as isize - 1) as usize];
    for y in 0..h as isize { for x in 0..w as isize {
        let (a, b, c) = (at(x - 1, y - 1), at(x, y - 1), at(x + 1, y - 1));
        let (d, e, f) = (at(x - 1, y), at(x, y), at(x + 1, y));
        let (g, hh, i) = (at(x - 1, y + 1), at(x, y + 1), at(x + 1, y + 1));
        let r: [u32; 9] = if b != hh && d != f {
            [
                if d == b { d } else { e },
                if (d == b && e != c) || (b == f && e != a) { b } else { e },
                if b == f { f } else { e },
                if (d == b && e != g) || (d == hh && e != a) { d } else { e },
                e,
                if (b == f && e != i) || (f == hh && e != c) { f } else { e },
                if d == hh { d } else { e },
                if (d == hh && e != i) || (f == hh && e != g) { hh } else { e },
                if f == hh { f } else { e },
            ]
        } else { [e; 9] };
        for k in 0..9 { out[(y as usize * 3 + k / 3) * ow + x as usize * 3 + k % 3] = r[k]; }
    } }
}
