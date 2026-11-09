//! Exécution « sans CPU 68000 » : le code Rust généré tourne seul ; ce module fournit le temps du matériel
//! (VDP, Z80, YM2612, PSG) et la livraison des interruptions entre deux blocs de code.
use crate::machine::Machine;
use md_core::{Console, MdBus};

impl Machine {
    /// Machine autonome : registres du 68000 dans `Machine`, mémoire et puces dans `con`.
    pub fn standalone(con: &mut Console, cb: fn(&mut Console) -> bool) -> Machine {
        let bus_ptr = con.bus_mut() as *mut MdBus;
        let con_ptr = con as *mut Console;
        let mut m = Machine::new(Vec::new());
        m.bus = Some(bus_ptr);
        m.con = Some(con_ptr);
        m.frame_cb = Some(cb);
        m.budget = i32::MAX / 2;
        m
    }

    #[inline]
    fn console(&self) -> &mut Console {
        // SAFETY: `standalone` garantit un pointeur valide pour toute la durée de vie de la machine.
        unsafe { &mut *self.con.expect("mode sans CPU") }
    }

    /// Fait avancer le matériel de `cy` cycles 68000 ; à la fin d'une image, appelle le callback de l'hôte.
    pub fn advance(&mut self, cy: u32) {
        let c = self.console();
        if c.nc_advance(cy) {
            if let Some(cb) = self.frame_cb {
                if !cb(c) { std::process::exit(0); }
            }
        }
    }

    /// Livre une interruption entre deux blocs si le VDP en demande une et que le masque le permet.
    /// `pc` = adresse de l'instruction qui s'exécuterait ensuite (adresse de retour empilée).
    #[inline]
    pub fn service(&mut self, pc: u32) {
        if self.con.is_some() { let _ = self.service_slow(pc); }
    }

    fn service_slow(&mut self, pc: u32) -> bool {
        let cy = self.cycles.replace(0);
        if cy > 0 { self.advance(cy); }
        let level = self.console().nc_irq_level();
        if level == 0 { return false; }
        let mask = ((self.sr >> 8) & 7) as u8;
        if level <= mask && level != 7 { return false; }
        // Prise en compte de l'interruption : comme le 68000, on empile PC puis SR et on passe en superviseur.
        self.console().nc_ack_irq();
        let old = self.sr;
        self.a[7] = self.a[7].wrapping_sub(4);
        self.wr32(self.a[7], pc);
        self.a[7] = self.a[7].wrapping_sub(2);
        self.wr16(self.a[7], old);
        self.sr = ((old | 0x2000) & !0x8700) | ((level as u16) << 8);
        let handler = self.rd32((24 + level as u32) * 4);
        match (crate::GEN.get().expect("code généré non enregistré").lookup)(handler) {
            Some(g) => {
                let r = g(self);
                if r != pc {
                    panic!("interruption niveau {level} : le gestionnaire {handler:#x} s'est terminé en {r:#x} au lieu de {pc:#x} (code non traduit ?)");
                }
            }
            None => panic!("gestionnaire d'interruption {handler:#x} non traduit"),
        }
        true
    }

    /// STOP : le « CPU » s'arrête jusqu'à la prochaine interruption acceptée (livrée par `service`).
    pub fn stop_wait(&mut self, next_pc: u32) {
        loop {
            let rem = self.console().nc_cycles_to_line_end();
            self.cycles.set(self.cycles.get() + rem);
            if self.service_slow(next_pc) { return; }
        }
    }

    /// Attente active détectée (le bloc reboucle sur lui-même sans rien écrire) : on saute à la fin de la ligne vidéo
    /// courante, ce qui fait avancer le matériel (VBlank, DMA, bus Z80...), puis on livre les interruptions.
    pub fn wait_idle(&mut self, pc: u32) {
        if self.con.is_none() { return; }
        let rem = self.console().nc_cycles_to_line_end();
        self.cycles.set(self.cycles.get() + rem);
        let _ = self.service_slow(pc);
    }
}
