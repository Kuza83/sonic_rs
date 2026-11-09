//! Outils de développement : fichier d'assets, recompilateur, balayage statique, désassemblage.
use crate::recomp;
use anyhow::{anyhow, Result};

/// Octets d'instructions atteintes depuis des points d'entrée fiables (vecteurs/objets, découvertes par exécution, anciennes routines à la main).
pub fn reliable_code_bytes(rom: &[u8]) -> std::collections::BTreeSet<u32> {
    let read = |f: &str| -> Vec<u32> { std::fs::read_to_string(f).unwrap_or_default().split(',').filter(|x| !x.trim().is_empty()).filter_map(|x| u32::from_str_radix(x.trim(), 16).ok()).collect() };
    let mut seeds = read("seeds.txt");
    seeds.extend(read("seeds_extra.txt"));
    seeds.extend(read("seeds_known.txt"));
    let entries: Vec<u32> = recomp::Recompiler::closure(rom, &seeds, 20000).into_iter().collect();
    recomp::Recompiler::new(rom, &entries).code_bytes()
}

/// Exécute l'outil demandé (`args` commence après le nom de programme). `None` si ce n'est pas un outil.
pub fn run(args: &mut std::iter::Peekable<impl Iterator<Item = String>>) -> Option<Result<()>> {
    let tool = args.peek()?.clone();
    if !["--make-assets", "--sweep", "--recompile", "--flow", "--disasm", "--upscale-pack"].contains(&tool.as_str()) { return None; }
    Some((|| -> Result<()> {
        if args.peek().map(String::as_str) == Some("--make-assets") {
            // --make-assets SORTIE [ROM] : écrit le fichier d'assets = la ROM dont les octets de CODE traduit (atteints par le flot de
            // contrôle depuis des points d'entrée fiables) sont mis à zéro. Les vecteurs et l'en-tête (0x000-0x1FF) sont conservés.
            args.next();
            let out = args.next().ok_or_else(|| anyhow!("usage: --make-assets SORTIE [ROM]"))?;
            let rom_path = args.next().unwrap_or_else(|| crate::host::rom_path());
            let rom = std::fs::read(&rom_path)?;
            // Points d'entrée fiables seulement : vecteurs/objets (seeds.txt), découvertes par exécution (seeds_extra.txt), routines à la main.
            // (seeds_sweep.txt est heuristique : il peut désigner des données, on ne s'en sert pas pour effacer des octets.)
            let code = reliable_code_bytes(&rom);
            // Octets de code finalement lus comme données (relevés par l'espion, assets_keep.txt) : on les conserve.
            let keep: std::collections::HashSet<u32> = std::fs::read_to_string("assets_keep.txt").unwrap_or_default().split_whitespace().filter_map(|x| u32::from_str_radix(x, 16).ok()).collect();
            let mut blob = rom.clone();
            let mut zeroed = 0usize;
            for &a in &code { if a >= 0x200 && (a as usize) < blob.len() && !keep.contains(&a) { blob[a as usize] = 0; zeroed += 1; } }
            // La ROM d'origine vérifie sa somme de contrôle au démarrage : on la recalcule pour le fichier d'assets (mot à 0x18E).
            let sum = blob[0x200..].chunks(2).fold(0u16, |acc, w| acc.wrapping_add(u16::from_be_bytes([w[0], *w.get(1).unwrap_or(&0)])));
            blob[0x18E] = (sum >> 8) as u8; blob[0x18F] = sum as u8;
            let entries = code.len();
            std::fs::write(&out, &blob)?;
            eprintln!("{} octets de code fiable, {} effacés ({} conservés car lus comme données) sur {} ({:.1} %) -> {}", entries, zeroed, keep.len(), rom.len(), 100.0 * zeroed as f64 / rom.len() as f64, out);
            return Ok(());
        }
        if args.peek().map(String::as_str) == Some("--sweep") {
            // --sweep DEBUT FIN (hex) : débuts probables de routines dans la région (balayage linéaire).
            args.next();
            let h = |x: Option<String>| u32::from_str_radix(x.unwrap().trim_start_matches("0x"), 16).unwrap();
            let (a, b) = (h(args.next()), h(args.next()));
            let rom = std::fs::read(&crate::host::rom_path())?;
            let v = recomp::Recompiler::sweep(&rom, a, b);
            println!("{}", v.iter().map(|x| format!("{x:x}")).collect::<Vec<_>>().join(","));
            return Ok(());
        }
        if args.peek().map(String::as_str) == Some("--recompile") {
            // --recompile a,b,c | known | closure:a,b,c [limite] : écrit le Rust généré sur la sortie standard.
            args.next();
            let spec = args.next().unwrap_or_default();
            let rom = std::fs::read(&crate::host::rom_path())?;
            let parse = |l: &str| -> Vec<u32> { l.split(',').filter(|x| !x.is_empty()).map(|x| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap()).collect() };
            let entries: Vec<u32> = if let Some(l) = spec.strip_prefix("closure:") {
                let limit = args.next().and_then(|v| v.parse().ok()).unwrap_or(5000);
                recomp::Recompiler::closure(&rom, &parse(l), limit).into_iter().collect()
            } else { parse(&spec) };
            eprintln!("{} routines à générer", entries.len());
            // Régions de la routine universelle : "universal:DEBUT-FIN,DEBUT-FIN" (hex) en dernier argument.
            let universal: Vec<(u32, u32)> = args.next().and_then(|a| a.strip_prefix("universal:").map(|l| l.to_string()))
                .map(|l| l.split(',').filter_map(|r| { let (a, b) = r.split_once('-')?; Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(b, 16).ok()?)) }).collect()).unwrap_or_default();
            print!("{}", recomp::Recompiler::new(&rom, &entries).with_universal(universal).generate());
            return Ok(());
        }
        if args.peek().map(String::as_str) == Some("--flow") {
            // --flow ADRESSE_HEX : instructions atteignables depuis ADRESSE (suit les branchements,
            // pas les bsr/jsr), puis la liste des appels. Sert à délimiter une vraie sous-routine.
            args.next();
            let start = u32::from_str_radix(args.next().unwrap().trim_start_matches("0x"), 16).unwrap();
            let rom = std::fs::read(&crate::host::rom_path())?;
            let mut seen = std::collections::BTreeMap::new();
            let mut calls = std::collections::BTreeSet::new();
            let mut todo = vec![start];
            let mut indirect = false;
            while let Some(pc) = todo.pop() {
                if seen.contains_key(&pc) || pc as usize + 2 > rom.len() { continue; }
                let d = m68k::disassemble(pc, |a| u16::from_be_bytes([rom[a as usize], rom[a as usize + 1]]));
                let t = d.text.clone();
                let mn = t.split_whitespace().next().unwrap_or("");
                let base = mn.split('.').next().unwrap_or("");
                let target = t.rsplit('$').next().and_then(|x| u32::from_str_radix(x.trim_end_matches(|c: char| !c.is_ascii_hexdigit()), 16).ok());
                let direct = t.contains(" $") && !t.contains('(');
                seen.insert(pc, t.clone());
                let next = pc + d.len;
                match base {
                    "rts" | "rte" | "rtr" => {}
                    "bra" | "jmp" => { if direct || (base == "jmp" && target.is_some() && t.contains(").l")) { if let Some(x) = target { todo.push(x); } } else { indirect = true; } }
                    "bsr" | "jsr" => { if let Some(x) = target { if direct || t.contains(").l") || t.contains(",pc)") { calls.insert(x); } else { indirect = true; } } else { indirect = true; } todo.push(next); }
                    b if b.starts_with('b') && b != "bchg" && b != "bclr" && b != "bset" && b != "btst" => { if let Some(x) = target { todo.push(x); } todo.push(next); }
                    "dbf" | "dbra" | "dbcc" | "dbeq" | "dbne" | "dbhi" | "dbmi" | "dbpl" | "dbt" => { if let Some(x) = target { todo.push(x); } todo.push(next); }
                    _ => todo.push(next),
                }
            }
            for (pc, t) in &seen { println!("{pc:06x}  {t}"); }
            println!("; octets {:#x}..{:#x} ({} instr.) appels: {:x?}{}", seen.keys().next().unwrap(), seen.keys().last().unwrap(), seen.len(), calls, if indirect { " [saut/appel indirect]" } else { "" });
            return Ok(());
        }
    // --upscale-pack DOSSIER N : écrit DOSSIER/tiles à partir de DOSSIER/originals par répétition de pixels (test du lot).
    if tool == "--upscale-pack" {
        args.next();
        let dir = args.next().ok_or_else(|| anyhow!("usage: --upscale-pack DOSSIER N"))?;
        let n: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(3);
        eprintln!("{} tuiles écrites", crate::pack::upscale_nearest(&dir, n)?);
    }
    if tool == "--disasm" {
        args.next();
        let hex = |s: Option<String>| u32::from_str_radix(s.unwrap().trim_start_matches("0x"), 16).unwrap();
        let (start, len) = (hex(args.next()), hex(args.next()));
        let rom = std::fs::read(crate::host::rom_path())?;
        let mut pc = start;
        while pc < start + len {
            let d = m68k::disassemble(pc, |a| u16::from_be_bytes([rom[a as usize], rom[a as usize + 1]]));
            println!("{pc:06x}  {}", d.text);
            pc += d.len.max(2);
        }
    }
    Ok(())
    })())
}
