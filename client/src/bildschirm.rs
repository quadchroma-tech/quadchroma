// Bildschirmwahl: die Liste der Bildschirme eines Hosts (MSG_BILDSCHIRME,
// Typ 12) und der Wunsch des Zuschauers (IN_BILDSCHIRM, Typ 70) als Bytes.
//
// Gemeinsam fuer den Client (liest die Liste, schreibt den Wunsch) und die
// Windows-Host-Rolle (schreibt die Liste, liest den Wunsch). Der Mac-Host
// baut dieselben Bytes in Objective-C (host/); massgeblich sind die
// Pruefvektoren im Test unten, die beide Seiten erfuellen muessen.
//
// Liste (Typ 12):
//   u8 fassung = 1, u8 anzahl (0..=16), u8 wunsch_laenge (0 = Automatik),
//   u8 frei, u8[wunsch_laenge] Kennung des Wunsches, dann je Eintrag:
//   u8 kennung_laenge (1..=64), u8 name_laenge (0..=48), u16 breite,
//   u16 hoehe, u16 hz (0 = unbekannt), u8 flags (Bit 0 Hauptbildschirm,
//   Bit 1 wird gestreamt), u8 frei, Kennung, Name (beide UTF-8).
// Wunsch (Typ 70):
//   u8 kennung_laenge (0 = Automatik), u8[kennung_laenge] Kennung.
//
// Die Kennung ist die stabile Kennung des Hosts (Mac: Vendor/Model/Serial,
// Windows: Geraete-ID des Monitors), nie ein Listenplatz. Gelesen wird
// defensiv: jede Laenge gegen den Rest geprueft, ungueltiges UTF-8 ersetzt,
// mehr als 16 Eintraege nur bis 16, eine fremde Fassung ergibt None.

#![allow(dead_code)]

/// Hoechstlaenge einer Kennung in Byte.
pub const KENNUNG_MAX: usize = 64;
/// Hoechstlaenge eines Namens in Byte.
pub const NAME_MAX: usize = 48;
/// Hoechstzahl der Eintraege in einer Liste.
pub const EINTRAEGE_MAX: usize = 16;
/// Fassung des Listenformats.
pub const FASSUNG: u8 = 1;
/// Bit 0 der Flags: Hauptbildschirm des Hosts.
pub const FLAG_HAUPT: u8 = 1;
/// Bit 1 der Flags: dieser Bildschirm wird gestreamt.
pub const FLAG_GESTREAMT: u8 = 2;

/// Ein Bildschirm des Hosts, wie er in der Liste steht.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BildschirmEintrag {
    pub kennung: String,
    pub name: String,
    pub breite: u16,
    pub hoehe: u16,
    pub hz: u16,
    pub haupt: bool,
    pub gestreamt: bool,
}

/// Die Liste samt Wunsch (None = Automatik, der Host folgt dem
/// Hauptbildschirm).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bildschirme {
    pub wunsch: Option<String>,
    pub eintraege: Vec<BildschirmEintrag>,
}

impl Bildschirme {
    /// Der Eintrag, der gerade gestreamt wird.
    pub fn gestreamt(&self) -> Option<&BildschirmEintrag> {
        self.eintraege.iter().find(|e| e.gestreamt)
    }

    /// Der gewuenschte Eintrag, falls er angeschlossen ist.
    pub fn gewuenschter(&self) -> Option<&BildschirmEintrag> {
        let w = self.wunsch.as_deref()?;
        self.eintraege.iter().find(|e| e.kennung == w)
    }
}

/// Kuerzt an einer Zeichengrenze auf hoechstens `max` Byte.
fn kuerzen(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut ende = max;
    while ende > 0 && !s.is_char_boundary(ende) {
        ende -= 1;
    }
    &s[..ende]
}

/// Eine Kennung, wie sie auf die Leitung darf: ohne Steuerzeichen, hoechstens
/// KENNUNG_MAX Byte. Leer bleibt leer (der Aufrufer nimmt dann einen Ersatz).
pub fn kennung_bereinigen(s: &str) -> String {
    let ohne: String = s.chars().filter(|c| !c.is_control()).collect();
    kuerzen(&ohne, KENNUNG_MAX).to_string()
}

/// Ein Name, wie er auf die Leitung darf: ohne Steuerzeichen, hoechstens
/// NAME_MAX Byte.
pub fn name_bereinigen(s: &str) -> String {
    let ohne: String = s.chars().filter(|c| !c.is_control()).collect();
    kuerzen(&ohne, NAME_MAX).to_string()
}

/// Die Liste als Nutzlast von Typ 12. Kennungen und Namen werden gekuerzt,
/// eine leere Kennung wird zu "?", mehr als EINTRAEGE_MAX Eintraege fallen weg.
pub fn bildschirme_kodieren(b: &Bildschirme) -> Vec<u8> {
    let wunsch = b.wunsch.as_deref().map(|w| kennung_bereinigen(w)).unwrap_or_default();
    let eintraege = &b.eintraege[..b.eintraege.len().min(EINTRAEGE_MAX)];
    let mut p = Vec::with_capacity(4 + wunsch.len() + eintraege.len() * 128);
    p.push(FASSUNG);
    p.push(eintraege.len() as u8);
    p.push(wunsch.len() as u8);
    p.push(0);
    p.extend_from_slice(wunsch.as_bytes());
    for e in eintraege {
        let mut kennung = kennung_bereinigen(&e.kennung);
        if kennung.is_empty() {
            kennung = "?".into();
        }
        let name = name_bereinigen(&e.name);
        p.push(kennung.len() as u8);
        p.push(name.len() as u8);
        p.extend_from_slice(&e.breite.to_le_bytes());
        p.extend_from_slice(&e.hoehe.to_le_bytes());
        p.extend_from_slice(&e.hz.to_le_bytes());
        let mut flags = 0u8;
        if e.haupt {
            flags |= FLAG_HAUPT;
        }
        if e.gestreamt {
            flags |= FLAG_GESTREAMT;
        }
        p.push(flags);
        p.push(0);
        p.extend_from_slice(kennung.as_bytes());
        p.extend_from_slice(name.as_bytes());
    }
    p
}

/// Liest die Nutzlast von Typ 12. None bei fremder Fassung oder wenn die
/// Bytes nicht bis zum Ende der angekuendigten Eintraege reichen (ein
/// abgeschnittener Eintrag ist ein Protokollfehler, kein Teilergebnis).
pub fn bildschirme_lesen(p: &[u8]) -> Option<Bildschirme> {
    if p.len() < 4 || p[0] != FASSUNG {
        return None;
    }
    let anzahl = (p[1] as usize).min(EINTRAEGE_MAX);
    let wl = p[2] as usize;
    let mut o = 4usize;
    if wl > KENNUNG_MAX || o + wl > p.len() {
        return None;
    }
    let wunsch = if wl == 0 { None } else { Some(String::from_utf8_lossy(&p[o..o + wl]).into_owned()) };
    o += wl;
    let mut eintraege = Vec::with_capacity(anzahl);
    for _ in 0..anzahl {
        if o + 10 > p.len() {
            return None;
        }
        let kl = p[o] as usize;
        let nl = p[o + 1] as usize;
        if kl == 0 || kl > KENNUNG_MAX || nl > NAME_MAX {
            return None;
        }
        let breite = u16::from_le_bytes([p[o + 2], p[o + 3]]);
        let hoehe = u16::from_le_bytes([p[o + 4], p[o + 5]]);
        let hz = u16::from_le_bytes([p[o + 6], p[o + 7]]);
        let flags = p[o + 8];
        o += 10;
        if o + kl + nl > p.len() {
            return None;
        }
        let kennung = String::from_utf8_lossy(&p[o..o + kl]).into_owned();
        let name = String::from_utf8_lossy(&p[o + kl..o + kl + nl]).into_owned();
        o += kl + nl;
        eintraege.push(BildschirmEintrag {
            kennung,
            name,
            breite,
            hoehe,
            hz,
            haupt: flags & FLAG_HAUPT != 0,
            gestreamt: flags & FLAG_GESTREAMT != 0,
        });
    }
    Some(Bildschirme { wunsch, eintraege })
}

/// Der Wunsch als Nutzlast von Typ 70 (None = Automatik).
pub fn wunsch_kodieren(wunsch: Option<&str>) -> Vec<u8> {
    let k = wunsch.map(kennung_bereinigen).unwrap_or_default();
    let mut p = Vec::with_capacity(1 + k.len());
    p.push(k.len() as u8);
    p.extend_from_slice(k.as_bytes());
    p
}

/// Liest die Nutzlast von Typ 70: Some(None) = Automatik, Some(Some(k)) =
/// Wunsch, None = ungueltig (Laenge passt nicht oder Steuerzeichen).
pub fn wunsch_lesen(p: &[u8]) -> Option<Option<String>> {
    let l = *p.first()? as usize;
    if l > KENNUNG_MAX || 1 + l > p.len() {
        return None;
    }
    if l == 0 {
        return Some(None);
    }
    let k = std::str::from_utf8(&p[1..1 + l]).ok()?;
    if k.chars().any(char::is_control) {
        return None;
    }
    Some(Some(k.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace().map(|b| u8::from_str_radix(b, 16).unwrap()).collect()
    }

    fn beispiel() -> Bildschirme {
        Bildschirme {
            wunsch: None,
            eintraege: vec![
                BildschirmEintrag {
                    kennung: "v1138-m1234-s0".into(),
                    name: "X27 X1".into(),
                    breite: 1920,
                    hoehe: 1080,
                    hz: 120,
                    haupt: true,
                    gestreamt: true,
                },
                BildschirmEintrag {
                    kennung: "v0-m0-s0".into(),
                    name: "Virtuell 16:9".into(),
                    breite: 1920,
                    hoehe: 1080,
                    hz: 240,
                    haupt: false,
                    gestreamt: false,
                },
            ],
        }
    }

    /// Die Pruefvektoren aus der Spezifikation (2.4), byte-genau; der
    /// Mac-Host muss dieselben Bytes erzeugen.
    #[test]
    fn pruefvektoren() {
        let liste = hex(
            "01 02 00 00 \
             0e 06 80 07 38 04 78 00 03 00 76 31 31 33 38 2d 6d 31 32 33 34 2d 73 30 58 32 37 20 58 31 \
             08 0d 80 07 38 04 f0 00 00 00 76 30 2d 6d 30 2d 73 30 56 69 72 74 75 65 6c 6c 20 31 36 3a 39",
        );
        assert_eq!(bildschirme_kodieren(&beispiel()), liste);
        assert_eq!(bildschirme_lesen(&liste), Some(beispiel()));
        assert_eq!(wunsch_kodieren(Some("v0-m0-s0")), hex("08 76 30 2d 6d 30 2d 73 30"));
        assert_eq!(wunsch_kodieren(None), vec![0]);
        assert_eq!(wunsch_lesen(&hex("08 76 30 2d 6d 30 2d 73 30")), Some(Some("v0-m0-s0".into())));
        assert_eq!(wunsch_lesen(&[0]), Some(None));
    }

    #[test]
    fn hin_und_zurueck_mit_wunsch_und_umlauten() {
        let mut b = beispiel();
        b.wunsch = Some("v0-m0-s0".into());
        b.eintraege[1].name = "Büro – Monitor ä".into();
        let p = bildschirme_kodieren(&b);
        assert_eq!(bildschirme_lesen(&p), Some(b.clone()));
        assert_eq!(bildschirme_lesen(&p).unwrap().gewuenschter().map(|e| e.hz), Some(240));
        assert_eq!(bildschirme_lesen(&p).unwrap().gestreamt().map(|e| e.name.as_str()), Some("X27 X1"));
    }

    #[test]
    fn leser_sind_defensiv() {
        let p = bildschirme_kodieren(&beispiel());
        // Abgeschnitten an jeder Stelle: nie Panik, nie ein Teilergebnis.
        for n in 0..p.len() {
            assert_eq!(bildschirme_lesen(&p[..n]), None, "Laenge {n}");
        }
        // Fremde Fassung.
        let mut f = p.clone();
        f[0] = 2;
        assert_eq!(bildschirme_lesen(&f), None);
        // Leere Liste mit Automatik.
        assert_eq!(bildschirme_lesen(&[1, 0, 0, 0]), Some(Bildschirme::default()));
        // Ungueltiges UTF-8 wird ersetzt, nicht abgelehnt.
        let mut u = p.clone();
        let name_start = 4 + 10 + 14;
        u[name_start] = 0xff;
        let g = bildschirme_lesen(&u).unwrap();
        assert!(g.eintraege[0].name.starts_with('\u{fffd}'));
        // Kennung darf nicht leer und nicht ueber 64 Byte sein.
        let mut k0 = p.clone();
        k0[4] = 0;
        assert_eq!(bildschirme_lesen(&k0), None);
        // Wunsch: Laenge ueber dem Rest, Steuerzeichen, zu lang.
        assert_eq!(wunsch_lesen(&[5, b'a']), None);
        assert_eq!(wunsch_lesen(&[2, b'a', b'\n']), None);
        assert_eq!(wunsch_lesen(&[]), None);
        let lang = vec![b'k'; 65];
        let mut w = vec![65u8];
        w.extend_from_slice(&lang);
        assert_eq!(wunsch_lesen(&w), None);
    }

    /// 17 angekuendigte und mitgeschickte Eintraege: der Leser nimmt die
    /// ersten 16 und uebergeht den Rest; Bytes hinter dem letzten Eintrag
    /// (ein spaeteres Format mit Anhang) stoeren nicht.
    #[test]
    fn leser_nimmt_hoechstens_16_und_uebergeht_anhang() {
        let mut b = beispiel();
        for i in 0..15 {
            let mut e = b.eintraege[1].clone();
            e.kennung = format!("v{i}-m{i}-s{i}");
            b.eintraege.push(e);
        }
        assert_eq!(b.eintraege.len(), 17);
        // Von Hand kodieren, denn der Kodierer selbst begrenzt schon auf 16.
        let mut p = vec![FASSUNG, 17, 0, 0];
        for e in &b.eintraege {
            p.push(e.kennung.len() as u8);
            p.push(e.name.len() as u8);
            p.extend_from_slice(&e.breite.to_le_bytes());
            p.extend_from_slice(&e.hoehe.to_le_bytes());
            p.extend_from_slice(&e.hz.to_le_bytes());
            p.push((e.haupt as u8) | ((e.gestreamt as u8) << 1));
            p.push(0);
            p.extend_from_slice(e.kennung.as_bytes());
            p.extend_from_slice(e.name.as_bytes());
        }
        let g = bildschirme_lesen(&p).unwrap();
        assert_eq!(g.eintraege.len(), EINTRAEGE_MAX);
        assert_eq!(g.eintraege[..], b.eintraege[..EINTRAEGE_MAX]);
        // Anhang hinter der Liste: uebergangen, nicht abgelehnt.
        let mut a = bildschirme_kodieren(&beispiel());
        a.extend_from_slice(&[9, 9, 9]);
        assert_eq!(bildschirme_lesen(&a), Some(beispiel()));
        // Ein 17. Eintrag, der abgeschnitten ist, stoert ebenfalls nicht:
        // gelesen werden nur 16.
        p.truncate(p.len() - 5);
        assert_eq!(bildschirme_lesen(&p).map(|g| g.eintraege.len()), Some(EINTRAEGE_MAX));
    }

    #[test]
    fn kodierer_kuerzt_und_begrenzt() {
        let mut b = beispiel();
        b.eintraege[0].kennung = "ä".repeat(40); // 80 Byte
        b.eintraege[0].name = "ü".repeat(30); // 60 Byte
        b.eintraege[1].kennung = "a\tb".into();
        b.wunsch = Some("x".repeat(100));
        for _ in 0..20 {
            b.eintraege.push(b.eintraege[1].clone());
        }
        let g = bildschirme_lesen(&bildschirme_kodieren(&b)).unwrap();
        assert_eq!(g.eintraege.len(), EINTRAEGE_MAX);
        assert_eq!(g.eintraege[0].kennung, "ä".repeat(32));
        assert_eq!(g.eintraege[0].name, "ü".repeat(24));
        assert_eq!(g.eintraege[1].kennung, "ab");
        assert_eq!(g.wunsch.as_deref().map(str::len), Some(KENNUNG_MAX));
        // Eine leere Kennung wird auf der Leitung zu "?".
        b.eintraege[1].kennung = "\u{1}".into();
        assert_eq!(bildschirme_lesen(&bildschirme_kodieren(&b)).unwrap().eintraege[1].kennung, "?");
    }
}
