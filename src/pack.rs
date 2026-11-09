//! Lot de tuiles de remplacement (dossier) : export des tuiles distinctes vues pendant le jeu, chargement des versions
//! améliorées, outil d'agrandissement simple pour les tests.
//!
//! Dossier : `originals/<hash>_<palette>.png` (8×8, couleurs de l'export, écrites par `export_seen`) et
//! `tiles/<hash>_<palette>.png` (`8N × 8N`, mêmes noms : la version améliorée, à fournir). Seules les tuiles ayant un fichier
//! dans `tiles/` sont remplacées ; toutes doivent avoir la même taille.

use crate::hd::{tile_hash, Pack, PackTile, TileKey};
use anyhow::{anyhow, Result};
use md_core::Console;
use std::collections::HashSet;

pub fn name(key: TileKey) -> String { format!("{:016x}_{}", key.0, key.1) }

/// Écrit dans `dir/originals` les tuiles (contenu, ligne de palette) affichées dans l'image courante et pas encore vues.
/// Le contenu est celui de la VRAM à la dernière ligne affichée, les couleurs celles de la CRAM à ce moment (intensité normale).
pub fn export_seen(c: &Console, dir: &str, seen: &mut HashSet<TileKey>) -> usize {
    let v = c.bus().vdp();
    let (vram, cram) = v.provenance_snapshot();
    if vram.is_empty() { return 0; }
    let prov = v.provenance();
    let (w, h) = (c.screen_width(), c.screen_height());
    let _ = std::fs::create_dir_all(format!("{dir}/originals"));
    let lvl = |x: u32| ((2 * x * 255 + 7) / 14) as u8;
    let mut new = 0;
    for y in 0..h { for x in 0..w {
        let p = prov[y * 320 + x];
        if (p >> 27) & 3 == 0 { continue; }
        let tile = (p & 0x7FF) as usize;
        let key = (tile_hash(&vram[tile * 32..tile * 32 + 32]), ((p >> 23) & 3) as u8);
        if !seen.insert(key) { continue; }
        let mut rgb = Vec::with_capacity(8 * 8 * 3);
        for i in 0..64 {
            let byte = vram[tile * 32 + (i / 8) * 4 + (i % 8) / 2];
            let ix = if i % 2 == 0 { byte >> 4 } else { byte & 15 } as usize;
            let wd = cram[key.1 as usize * 16 + ix] as u32;
            rgb.extend_from_slice(&[lvl((wd >> 1) & 7), lvl((wd >> 5) & 7), lvl((wd >> 9) & 7)]);
        }
        crate::host::write_png(&format!("{dir}/originals/{}.png", name(key)), 8, 8, &rgb);
        new += 1;
    } }
    new
}

fn read_png(path: &std::path::Path) -> Result<(usize, usize, Vec<u32>)> {
    let mut reader = png::Decoder::new(std::fs::File::open(path)?).read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let (w, h) = (info.width as usize, info.height as usize);
    let px = match (info.color_type, info.bit_depth) {
        (png::ColorType::Rgb, png::BitDepth::Eight) => buf.chunks(3).map(|p| ((p[0] as u32) << 24) | ((p[1] as u32) << 16) | ((p[2] as u32) << 8) | 0xFF).collect(),
        (png::ColorType::Rgba, png::BitDepth::Eight) => buf.chunks(4).map(|p| ((p[0] as u32) << 24) | ((p[1] as u32) << 16) | ((p[2] as u32) << 8) | 0xFF).collect(),
        (png::ColorType::Grayscale, png::BitDepth::Eight) => buf.iter().map(|&g| ((g as u32) << 24) | ((g as u32) << 16) | ((g as u32) << 8) | 0xFF).collect(),
        other => return Err(anyhow!("{} : format PNG non géré {:?} (utiliser RGB ou RGBA 8 bits)", path.display(), other)),
    };
    Ok((w, h, px))
}

/// Charge `dir/tiles` (versions améliorées) avec `dir/originals` (références). Ignore les tuiles sans original ou de taille incohérente.
pub fn load(dir: &str) -> Result<Pack> {
    let mut pack = Pack::default();
    let (mut ignored, mut first_err) = (0, None);
    for entry in std::fs::read_dir(format!("{dir}/tiles"))? {
        let path = entry?.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let Some((hash, pal)) = stem.split_once('_').and_then(|(h, p)| Some((u64::from_str_radix(h, 16).ok()?, p.parse::<u8>().ok()?))) else { continue };
        let result = (|| -> Result<PackTile> {
            let (w, h, hd) = read_png(&path)?;
            if w != h || w % 8 != 0 { return Err(anyhow!("{} : {w}×{h} (carré multiple de 8 attendu)", path.display())); }
            let scale = w / 8;
            if pack.scale != 0 && pack.scale != scale { return Err(anyhow!("{} : échelle {scale} différente de {}", path.display(), pack.scale)); }
            let (ow, oh, orig) = read_png(&std::path::PathBuf::from(format!("{dir}/originals/{stem}.png")))?;
            if (ow, oh) != (8, 8) { return Err(anyhow!("original {stem} : {ow}×{oh} au lieu de 8×8")); }
            let reference: Vec<u32> = (0..w * w).map(|i| orig[(i / w / scale) * 8 + (i % w) / scale]).collect();
            pack.scale = scale;
            Ok(PackTile::new(&hd, &reference))
        })();
        match result {
            Ok(t) => { pack.tiles.insert((hash, pal), t); }
            Err(e) => { ignored += 1; first_err.get_or_insert(e.to_string()); }
        }
    }
    if ignored > 0 { eprintln!("lot : {ignored} tuile(s) ignorée(s), par exemple : {}", first_err.unwrap()); }
    eprintln!("lot : {} tuiles améliorées chargées (échelle {})", pack.tiles.len(), pack.scale);
    Ok(pack)
}

/// Outil de test : écrit dans `dir/tiles` chaque original agrandi par simple répétition de pixels (aucune amélioration).
pub fn upscale_nearest(dir: &str, scale: usize) -> Result<usize> {
    std::fs::create_dir_all(format!("{dir}/tiles"))?;
    let mut n = 0;
    for entry in std::fs::read_dir(format!("{dir}/originals"))? {
        let path = entry?.path();
        let (_, _, orig) = read_png(&path)?;
        let w = 8 * scale;
        let mut rgb = Vec::with_capacity(w * w * 3);
        for i in 0..w * w {
            let p = orig[(i / w / scale) * 8 + (i % w) / scale];
            rgb.extend_from_slice(&[(p >> 24) as u8, (p >> 16) as u8, (p >> 8) as u8]);
        }
        crate::host::write_png(&format!("{dir}/tiles/{}", path.file_name().unwrap().to_string_lossy()), w, w, &rgb);
        n += 1;
    }
    Ok(n)
}
