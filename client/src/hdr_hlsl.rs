// HDR-Shader in HLSL fuer die Windows-Seiten: der Wandler des Hosts
// (host/wandler.rs) - und die Anzeige des Clients (anzeige.rs), sobald sie
// HDR-Stroeme darstellt. Die Rechnung ist die aus dem HDR-Plan, Abschnitt 4:
// Arbeitsraum ist lineares Licht relativ zum SDR-Weiss der Quelle (rel 1.0 =
// SDR-Weiss des Hosts), auf SDR-Zielen wird farbtontreu abgeschnitten
// (Entscheidung E4), ausgegeben wird stueckweise sRGB.
//
// Eine Quelle je Nutzer: die gemeinsamen Funktionen stehen in
// hlsl_gemeinsam!, jeder Nutzer setzt seine Einstiege mit concat! dahinter -
// D3DCompile bekommt EINEN Text, ohne #include. Das Gegenstueck in Rust
// (`spiegel`, nur im Test) rechnet dieselben Schritte in f32; die Tests
// halten die Zahlen im HLSL-Text und die Lage der Konstanten gegen Rust fest.

#![cfg_attr(not(windows), allow(dead_code))]

/// Die gemeinsamen HLSL-Funktionen (reiner Text; als Makro, damit concat! ihn
/// mit den Einstiegen eines Nutzers zu einer Quelle verbindet).
macro_rules! hlsl_gemeinsam {
    () => {
        r#"
// ---------- gemeinsam: ein Dreieck ueber das ganze Ziel, ohne Vertexpuffer ----------
// (0,0) (2,0) (0,2) im Bildraum, nach Clip-Koordinaten gespiegelt (y nach unten).
float4 vs_voll(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p * float2(2, -2) + float2(-1, 1), 0, 1);
}

// Farbtontreu abschneiden (E4): liegt das groesste Glied ueber hd, wird es
// auf hd gesetzt und die anderen im selben Verhaeltnis - der Farbton bleibt,
// nur die Helligkeit wird begrenzt. Bis hd bleibt alles unberuehrt.
float3 farbtontreu_abschneiden(float3 rgb, float hd) {
    float m = max(rgb.r, max(rgb.g, rgb.b));
    return (m > hd) ? rgb * (hd / m) : rgb;
}

// sRGB-OETF, stueckweise (IEC 61966-2-1): linear 0..1 -> kodiert 0..1.
// exp2(log2()) statt pow: dasselbe, aber ohne die Warnung zu negativen
// Basen (log2(0) = -inf, exp2(-inf) = 0; der Zweig ist dort ohnehin der tiefe).
float3 srgb_oetf(float3 l) {
    l = saturate(l);
    float3 tief = l * 12.92;
    float3 hoch = 1.055 * exp2(log2(l) * (1.0 / 2.4)) - 0.055;
    return (l <= 0.0031308) ? tief : hoch;
}

// 8 Bit: selbst runden und den exakten Bruch k/255 ausgeben - die UNORM-
// Wandlung des Ziels trifft dann genau k (an der Grenze .5 darf sie laut
// D3D-Spezifikation danebenliegen, ein eigener Bruch nicht).
float3 acht_bit(float3 v) {
    return floor(saturate(v) * 255.0 + 0.5) * (1.0 / 255.0);
}
"#
    };
}

/// Der Wandler des Hosts (host/wandler.rs), Modus SDR: ein HDR-Desktop
/// (FP16, scRGB) wird SDR (sRGB, BGRA8). Einstiege vs_voll und ps_sdr;
/// Register b0 und t0.
pub(crate) const WANDLER: &str = concat!(
    hlsl_gemeinsam!(),
    r#"
// ---------- Wandler, Modus SDR: HDR-Desktop (scRGB, FP16) -> SDR (sRGB, BGRA8) ----------
cbuffer Wandler : register(b0) {
    float  weiss_kehrwert;   // 1000 / SDRWhiteLevel: scRGB -> relativ zum SDR-Weiss des Hosts
    float3 _w;
};
Texture2D<float4> eingang : register(t0);   // FP16-Kopie der Duplication: scRGB, linear, BT.709-Primaerfarben
float4 ps_sdr(float4 pos : SV_Position) : SV_Target {
    // scRGB 1.0 = 80 nit; rel 1.0 = SDR-Weiss des Hosts. Negative Anteile
    // (Farben ausserhalb von BT.709) fallen weg, NaN ebenso (max liefert 0).
    float3 rel = max(eingang.Load(int3(pos.xy, 0)).rgb * weiss_kehrwert, 0.0);
    // Ziel SDR: Kopfraum 1, Knie 1 -> farbtontreu auf SDR-Weiss abschneiden.
    return float4(acht_bit(srgb_oetf(farbtontreu_abschneiden(rel, 1.0))), 1.0);
}
"#
);

/// Konstanten des Wandlers (cbuffer Wandler, 16 Byte).
#[repr(C)]
pub(crate) struct KonstWandler {
    /// 1000 / SDRWhiteLevel: scRGB -> relativ zum SDR-Weiss des Hosts.
    pub weiss_kehrwert: f32,
    pub _w: [f32; 3],
}

/// Die Shader-Rechnung in Rust, Schritt fuer Schritt wie im HLSL (f32), fuer
/// die Tests auf allen Plattformen und fuer die Karten-Tests des Wandlers.
/// Dazu FP16 wie auf der Karte (runden zur naechsten, bei Gleichstand gerade).
#[cfg(test)]
pub(crate) mod spiegel {
    /// Die Zahlen der sRGB-Kurve, wie sie im HLSL-Text stehen.
    pub const SRGB_KNICK: f32 = 0.0031308;
    pub const SRGB_STEIGUNG: f32 = 12.92;
    pub const SRGB_FAKTOR: f32 = 1.055;
    pub const SRGB_VERSATZ: f32 = 0.055;
    pub const SRGB_EXPONENT: f32 = 2.4;

    pub fn farbtontreu_abschneiden(rgb: [f32; 3], hd: f32) -> [f32; 3] {
        let m = rgb[0].max(rgb[1]).max(rgb[2]);
        if m > hd {
            rgb.map(|c| c * (hd / m))
        } else {
            rgb
        }
    }

    pub fn srgb_oetf(l: f32) -> f32 {
        let l = if l.is_nan() { 0.0 } else { l.clamp(0.0, 1.0) };
        if l <= SRGB_KNICK {
            l * SRGB_STEIGUNG
        } else {
            SRGB_FAKTOR * l.powf(1.0 / SRGB_EXPONENT) - SRGB_VERSATZ
        }
    }

    /// Die Umkehrung (sRGB-Inhalt -> linear): so legt Windows SDR-Inhalt in
    /// den scRGB-Desktop, bevor es ihn mit dem SDR-Weiss multipliziert.
    pub fn srgb_eotf(v: f32) -> f32 {
        if v <= SRGB_KNICK * SRGB_STEIGUNG {
            v / SRGB_STEIGUNG
        } else {
            ((v + SRGB_VERSATZ) / SRGB_FAKTOR).powf(SRGB_EXPONENT)
        }
    }

    pub fn acht_bit(v: f32) -> u8 {
        (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8
    }

    /// ps_sdr: ein scRGB-Punkt -> 8-Bit-sRGB.
    pub fn sdr_aus_scrgb(scrgb: [f32; 3], weiss_kehrwert: f32) -> [u8; 3] {
        // f32::max liefert bei NaN das andere Glied - wie max in HLSL.
        let rel = scrgb.map(|c| (c * weiss_kehrwert).max(0.0));
        farbtontreu_abschneiden(rel, 1.0).map(|c| acht_bit(srgb_oetf(c)))
    }

    /// f32 -> FP16-Bits, zur naechsten gerundet (bei Gleichstand gerade),
    /// mit Subnormalen, Ueberlauf nach unendlich.
    pub fn halb_bits(x: f32) -> u16 {
        let b = x.to_bits();
        let vz = ((b >> 16) & 0x8000) as u16;
        let exp = ((b >> 23) & 0xff) as i32;
        let mant = b & 0x7f_ffff;
        if exp == 0xff {
            return vz | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
        }
        let e = exp - 127 + 15;
        if e >= 0x1f {
            return vz | 0x7c00;
        }
        if e <= 0 {
            // Subnormal: Einheit 2^-24, Wert = (1.mant) * 2^(exp-127).
            let schieb = (126 - exp) as u32;
            if schieb > 24 {
                return vz;
            }
            let m = mant | 0x80_0000;
            let mut r = m >> schieb;
            let rest = m & ((1 << schieb) - 1);
            let halb = 1 << (schieb - 1);
            if rest > halb || (rest == halb && r & 1 == 1) {
                r += 1;
            }
            return vz | r as u16;
        }
        let mut h = ((e as u32) << 10) | (mant >> 13);
        let rest = mant & 0x1fff;
        if rest > 0x1000 || (rest == 0x1000 && h & 1 == 1) {
            h += 1; // ein Uebertrag in den Exponenten ist richtig (bis unendlich)
        }
        vz | h as u16
    }

    pub fn aus_halb(h: u16) -> f32 {
        let vz = if h & 0x8000 != 0 { -1.0f32 } else { 1.0 };
        let e = ((h >> 10) & 0x1f) as i32;
        let m = (h & 0x3ff) as f32;
        match e {
            0 => vz * m * 2f32.powi(-24),
            0x1f if m == 0.0 => vz * f32::INFINITY,
            0x1f => f32::NAN,
            _ => vz * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
        }
    }

    /// Ein Wert, wie er nach FP16 und zurueck aussieht.
    pub fn halb(x: f32) -> f32 {
        aus_halb(halb_bits(x))
    }
}

#[cfg(test)]
mod tests {
    use super::spiegel::*;
    use super::*;

    /// Die Konstanten liegen so, wie der Shader sie liest.
    #[test]
    fn konstanten_wie_im_shader() {
        assert_eq!(std::mem::size_of::<KonstWandler>(), 16);
        assert_eq!(std::mem::offset_of!(KonstWandler, weiss_kehrwert), 0);
        // Reihenfolge im cbuffer wie im struct.
        let cb = &WANDLER[WANDLER.find("cbuffer Wandler").expect("cbuffer Wandler")..];
        let a = cb.find("weiss_kehrwert").unwrap();
        let b = cb.find("_w;").unwrap();
        assert!(a < b);
    }

    /// Die Zahlen der sRGB-Kurve im HLSL sind die aus Rust.
    #[test]
    fn srgb_zahlen_wie_in_rust() {
        for (name, z) in [
            ("Knick", SRGB_KNICK),
            ("Steigung", SRGB_STEIGUNG),
            ("Faktor", SRGB_FAKTOR),
            ("Versatz", SRGB_VERSATZ),
            ("Exponent", SRGB_EXPONENT),
        ] {
            let text = format!("{z}");
            assert!(WANDLER.contains(&text), "{name} {text} fehlt im HLSL");
        }
        assert!(WANDLER.contains("(l <= 0.0031308) ? tief : hoch"));
        assert!(WANDLER.contains("1.0 / 2.4"));
    }

    /// Die Einstiege, die der Wandler uebersetzt, stehen im Text - je einmal.
    #[test]
    fn einstiege_vorhanden() {
        for e in ["float4 vs_voll(", "float4 ps_sdr(", "float3 farbtontreu_abschneiden(", "float3 srgb_oetf(", "float3 acht_bit("] {
            assert_eq!(WANDLER.matches(e).count(), 1, "{e}");
        }
    }

    /// FP16 wie auf der Karte: bekannte Bitmuster, Rundung, Subnormale.
    #[test]
    fn halb_wie_ieee() {
        for (x, h) in [
            (0.0f32, 0x0000u16),
            (1.0, 0x3c00),
            (0.5, 0x3800),
            (2.5, 0x4100),
            (12.5, 0x4a40),
            (65504.0, 0x7bff),
            (1.0 / 3.0, 0x3555),
            (6.103_515_6e-5, 0x0400),
            (5.960_464_5e-8, 0x0001),
            (-2.0, 0xc000),
            (1e6, 0x7c00),
        ] {
            assert_eq!(halb_bits(x), h, "{x}");
        }
        // Gleichstand rundet gerade: 1 + 2^-11 -> 1, 1 + 3*2^-11 -> 1 + 2^-9.
        assert_eq!(halb_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        assert_eq!(halb_bits(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        for h in [0x0001u16, 0x03ff, 0x0400, 0x3c00, 0x4a40, 0x7bff] {
            assert_eq!(halb_bits(aus_halb(h)), h);
        }
    }

    /// Der Kern von Schritt 2 (HDR-Desktop als SDR): SDR-Inhalt, wie Windows
    /// ihn in den scRGB-Desktop legt (sRGB -> linear, mal SDR-Weiss, FP16),
    /// kommt mit genau seinen 8-Bit-Werten heraus - bei jedem SDR-Weiss.
    #[test]
    fn sdr_inhalt_kommt_exakt_heraus() {
        let mut abweichungen = Vec::new();
        for level in [1000u32, 2500, 3000, 5000] {
            let weiss = level as f32 / 1000.0;
            let kehrwert = 1000.0 / level as f32;
            for v in 0..=255u32 {
                let rgb = [v, 255 - v, (v * 7) % 256];
                let scrgb = rgb.map(|k| halb(srgb_eotf(k as f32 / 255.0) * weiss));
                let aus = sdr_aus_scrgb(scrgb, kehrwert);
                let soll = rgb.map(|k| k as u8);
                if aus != soll {
                    abweichungen.push((level, rgb, aus));
                }
            }
        }
        assert!(abweichungen.is_empty(), "{} Abweichungen, erste: {:?}", abweichungen.len(), &abweichungen[..abweichungen.len().min(5)]);
    }

    /// Heller als SDR-Weiss: farbtontreu abgeschnitten - das groesste Glied
    /// wird Weiss, die anderen behalten ihr Verhaeltnis; Grau wird Weiss.
    /// Negatives und NaN werden 0.
    #[test]
    fn heller_als_sdr_weiss_farbtontreu() {
        // 1000 nit bei SDR-Weiss 80 nit: scRGB 12.5.
        assert_eq!(sdr_aus_scrgb([12.5, 12.5, 12.5], 1.0), [255, 255, 255]);
        // (12.5, 2.5, 0) -> rel (1, 0.2, 0): Verhaeltnis 5:1 bleibt.
        let r = sdr_aus_scrgb([12.5, 2.5, 0.0], 1.0);
        assert_eq!(r, [255, acht_bit(srgb_oetf(0.2)), 0]);
        // Bei SDR-Weiss 400 nit (5.0) ist 12.5 nur 2.5-fach ueber Weiss.
        let r = sdr_aus_scrgb([12.5, 5.0, 2.5], 0.2);
        assert_eq!(r, [255, acht_bit(srgb_oetf(0.4)), acht_bit(srgb_oetf(0.2))]);
        // Bis SDR-Weiss unberuehrt.
        assert_eq!(farbtontreu_abschneiden([1.0, 0.5, 0.25], 1.0), [1.0, 0.5, 0.25]);
        // Negativ (ausserhalb BT.709) und NaN -> 0.
        assert_eq!(sdr_aus_scrgb([-0.5, 0.5, f32::NAN], 1.0), [0, acht_bit(srgb_oetf(0.5)), 0]);
    }
}
