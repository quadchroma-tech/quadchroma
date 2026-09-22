//! H.264-Sequenzparametersatz (SPS) um ein VUI mit bitstream_restriction
//! ergaenzen - fuer den NVDEC-Pfad.
//!
//! Der Mac-Host codiert mit VideoToolbox ohne Bildumsortierung
//! (AllowFrameReordering=false, MaxFrameDelayCount=1), schreibt das aber
//! nicht ins SPS: es hat kein VUI (vui_parameters_present_flag = 0). Ein
//! Decoder, der nur den Strom sieht, muss dann vom Schlimmsten ausgehen. Der
//! cuvid-Parser (NVDEC) tut genau das und haelt die groesste Umsortierungs-
//! tiefe des Levels zurueck - rund 16 Bilder, gemessen 150-290 ms Decoder-
//! zeit. FFmpegs eigener h264-Decoder ist nicht betroffen (er schaetzt aus
//! dem Strom), HEVC auch nicht (dort steht die Tiefe im SPS selbst).
//!
//! Abhilfe: in jedem SPS ohne VUI eines anlegen, das nur
//! bitstream_restriction_flag=1 mit max_num_reorder_frames=0 traegt (H.264
//! Anhang E.1.1). Alles vor dem Flag wird bitgenau uebernommen; ein SPS,
//! das schon ein VUI hat, bleibt unangetastet - was drin steht, wissen wir
//! dann nicht, und der Encoder hatte einen Grund. Ebenso bleibt ein SPS mit
//! Skalierungsmatrizen liegen (die lesen wir nicht; VideoToolbox schreibt
//! keine).

/// Liest Bits aus einer RBSP (Nutzlast OHNE Emulationsschutz).
struct BitLeser<'a> {
    daten: &'a [u8],
    /// Position in Bits.
    pos: usize,
}

impl<'a> BitLeser<'a> {
    fn neu(daten: &'a [u8]) -> Self {
        BitLeser { daten, pos: 0 }
    }

    /// Ein Bit; None hinter dem Ende.
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.daten.get(self.pos / 8)?;
        let b = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(b as u32)
    }

    /// n Bits (n <= 32), hoechstwertiges zuerst.
    fn u(&mut self, n: u32) -> Option<u32> {
        let mut w = 0u32;
        for _ in 0..n {
            w = (w << 1) | self.bit()?;
        }
        Some(w)
    }

    fn u1(&mut self) -> Option<bool> {
        Some(self.bit()? == 1)
    }

    /// Vorzeichenloser Exp-Golomb-Wert (ue(v)).
    fn ue(&mut self) -> Option<u32> {
        let mut nullen = 0u32;
        while self.bit()? == 0 {
            nullen += 1;
            if nullen > 31 {
                return None;
            }
        }
        let rest = self.u(nullen)?;
        Some((1u32 << nullen) - 1 + rest)
    }

    /// Vorzeichenbehafteter Exp-Golomb-Wert (se(v)).
    fn se(&mut self) -> Option<i32> {
        let k = self.ue()? as i64;
        let w = if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) };
        Some(w as i32)
    }
}

/// Schreibt Bits in eine RBSP (ohne Emulationsschutz; den legt `schuetzen`
/// hinterher).
struct BitSchreiber {
    bytes: Vec<u8>,
    /// Anzahl geschriebener Bits.
    bits: usize,
}

impl BitSchreiber {
    fn neu() -> Self {
        BitSchreiber { bytes: Vec::new(), bits: 0 }
    }

    fn bit(&mut self, b: u32) {
        if self.bits % 8 == 0 {
            self.bytes.push(0);
        }
        if b & 1 == 1 {
            let i = self.bits / 8;
            self.bytes[i] |= 1 << (7 - self.bits % 8);
        }
        self.bits += 1;
    }

    /// n Bits (n <= 32), hoechstwertiges zuerst.
    fn u(&mut self, n: u32, wert: u32) {
        for i in (0..n).rev() {
            self.bit((wert >> i) & 1);
        }
    }

    fn u1(&mut self, b: bool) {
        self.bit(b as u32);
    }

    /// Vorzeichenloser Exp-Golomb-Wert.
    fn ue(&mut self, wert: u32) {
        let w = wert as u64 + 1;
        let laenge = 64 - w.leading_zeros(); // Bits von w, mindestens 1
        self.u(laenge - 1, 0);
        self.u(laenge, w as u32);
    }

    /// Vorzeichenbehafteter Exp-Golomb-Wert.
    #[cfg_attr(not(test), allow(dead_code))]
    fn se(&mut self, wert: i32) {
        let k = if wert > 0 { 2 * wert as i64 - 1 } else { -2 * wert as i64 };
        self.ue(k as u32);
    }

    /// Die ersten `n` Bits eines anderen Puffers unveraendert uebernehmen.
    fn kopie(&mut self, quelle: &[u8], n: usize) {
        let mut l = BitLeser::neu(quelle);
        for _ in 0..n {
            self.bit(l.bit().unwrap_or(0));
        }
    }

    /// rbsp_trailing_bits: eine Eins, dann Nullen bis zur Bytegrenze.
    fn abschluss(mut self) -> Vec<u8> {
        self.bit(1);
        while self.bits % 8 != 0 {
            self.bit(0);
        }
        self.bytes
    }
}

/// Emulationsschutz entfernen: aus 00 00 03 wird 00 00. Die Nutzlast ist
/// die des NAL ohne das Kopfbyte.
fn entschuetzen(nutzlast: &[u8]) -> Vec<u8> {
    let mut raus = Vec::with_capacity(nutzlast.len());
    let mut nullen = 0usize;
    for &b in nutzlast {
        if nullen >= 2 && b == 3 {
            nullen = 0;
            continue;
        }
        raus.push(b);
        nullen = if b == 0 { nullen + 1 } else { 0 };
    }
    raus
}

/// Emulationsschutz einfuegen: vor jedem Byte 00..03 hinter zwei Nullen ein
/// 03. Eine RBSP endet mit dem Abschlussbit, also nie auf 00 - der Sonderfall
/// "03 am Ende" (7.4.1) kommt nicht vor.
fn schuetzen(rbsp: &[u8]) -> Vec<u8> {
    let mut raus = Vec::with_capacity(rbsp.len() + 4);
    let mut nullen = 0usize;
    for &b in rbsp {
        if nullen >= 2 && b <= 3 {
            raus.push(3);
            nullen = 0;
        }
        raus.push(b);
        nullen = if b == 0 { nullen + 1 } else { 0 };
    }
    raus
}

/// Was aus dem SPS gebraucht wird: die Bitposition des
/// vui_parameters_present_flag, sein Wert, und ein paar Felder fuer die
/// Protokollzeile und die Tests.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpsFelder {
    profile_idc: u32,
    level_idc: u32,
    max_num_ref_frames: u32,
    /// pic_width_in_mbs_minus1 + 1
    breite_mb: u32,
    /// pic_height_in_map_units_minus1 + 1
    hoehe_einheiten: u32,
    frame_cropping: Option<[u32; 4]>,
    /// Bitposition des vui_parameters_present_flag in der RBSP.
    vui_flag_pos: usize,
    vui_present: bool,
}

/// Profile mit den zusaetzlichen Feldern (chroma_format_idc usw.) nach
/// 7.3.2.1.1.
const PROFILE_MIT_CHROMA: [u32; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

/// Das SPS bis einschliesslich vui_parameters_present_flag lesen. None bei
/// einem Strom, der nicht aufgeht, oder bei Skalierungsmatrizen (die
/// ueberspringen wir nicht).
fn sps_lesen(rbsp: &[u8]) -> Option<SpsFelder> {
    let mut l = BitLeser::neu(rbsp);
    let profile_idc = l.u(8)?;
    let _constraint = l.u(8)?;
    let level_idc = l.u(8)?;
    let _sps_id = l.ue()?;
    if PROFILE_MIT_CHROMA.contains(&profile_idc) {
        let chroma_format_idc = l.ue()?;
        if chroma_format_idc == 3 {
            let _separate_colour_plane = l.u1()?;
        }
        let _bit_depth_luma_minus8 = l.ue()?;
        let _bit_depth_chroma_minus8 = l.ue()?;
        let _qpprime_y_zero_transform_bypass = l.u1()?;
        let seq_scaling_matrix_present = l.u1()?;
        if seq_scaling_matrix_present {
            return None;
        }
    }
    let _log2_max_frame_num_minus4 = l.ue()?;
    let pic_order_cnt_type = l.ue()?;
    match pic_order_cnt_type {
        0 => {
            let _log2_max_pic_order_cnt_lsb_minus4 = l.ue()?;
        }
        1 => {
            let _delta_pic_order_always_zero = l.u1()?;
            let _offset_for_non_ref_pic = l.se()?;
            let _offset_for_top_to_bottom_field = l.se()?;
            let n = l.ue()?;
            if n > 255 {
                return None;
            }
            for _ in 0..n {
                let _offset_for_ref_frame = l.se()?;
            }
        }
        _ => {}
    }
    let max_num_ref_frames = l.ue()?;
    let _gaps_in_frame_num_allowed = l.u1()?;
    let breite_mb = l.ue()? + 1;
    let hoehe_einheiten = l.ue()? + 1;
    let frame_mbs_only = l.u1()?;
    if !frame_mbs_only {
        let _mb_adaptive_frame_field = l.u1()?;
    }
    let _direct_8x8_inference = l.u1()?;
    let frame_cropping = if l.u1()? {
        Some([l.ue()?, l.ue()?, l.ue()?, l.ue()?])
    } else {
        None
    };
    let vui_flag_pos = l.pos;
    let vui_present = l.u1()?;
    Some(SpsFelder {
        profile_idc, level_idc, max_num_ref_frames, breite_mb, hoehe_einheiten,
        frame_cropping, vui_flag_pos, vui_present,
    })
}

/// NAL-Typ SPS in H.264.
const NAL_SPS: u8 = 7;

/// Ein SPS-NAL (nal[0] = Kopfbyte, danach die geschuetzte Nutzlast) mit
/// einem VUI, das bitstream_restriction_flag=1 und max_num_reorder_frames=0
/// traegt. None, wenn es kein SPS ist, nicht aufgeht, Skalierungsmatrizen
/// traegt oder schon ein VUI hat - dann bleibt das Original.
pub fn h264_sps_mit_vui(nal: &[u8]) -> Option<Vec<u8>> {
    let (&kopf, nutzlast) = nal.split_first()?;
    if kopf & 0x1f != NAL_SPS {
        return None;
    }
    let rbsp = entschuetzen(nutzlast);
    let felder = sps_lesen(&rbsp)?;
    if felder.vui_present {
        return None;
    }
    let mut s = BitSchreiber::neu();
    s.kopie(&rbsp, felder.vui_flag_pos);
    s.u1(true); // vui_parameters_present_flag
    // vui_parameters() nach E.1.1 - alles aus, bis auf die Begrenzung.
    s.u1(false); // aspect_ratio_info_present_flag
    s.u1(false); // overscan_info_present_flag
    s.u1(false); // video_signal_type_present_flag
    s.u1(false); // chroma_loc_info_present_flag
    s.u1(false); // timing_info_present_flag
    s.u1(false); // nal_hrd_parameters_present_flag
    s.u1(false); // vcl_hrd_parameters_present_flag
    s.u1(false); // pic_struct_present_flag
    s.u1(true); // bitstream_restriction_flag
    s.u1(true); // motion_vectors_over_pic_boundaries_flag
    s.ue(2); // max_bytes_per_pic_denom
    s.ue(1); // max_bits_per_mb_denom
    s.ue(16); // log2_max_mv_length_horizontal
    s.ue(16); // log2_max_mv_length_vertical
    s.ue(0); // max_num_reorder_frames
    s.ue(felder.max_num_ref_frames); // max_dec_frame_buffering
    let neu = s.abschluss();
    let mut raus = Vec::with_capacity(neu.len() + 5);
    raus.push(kopf);
    raus.extend_from_slice(&schuetzen(&neu));
    Some(raus)
}

/// Positionen der Startcodes 00 00 01 in einer Annex-B-Einheit (die
/// fuehrende Null eines 00 00 00 01 gehoert zum Ende des Vorgaengers).
fn startcodes(au: &[u8]) -> Vec<usize> {
    let mut raus = Vec::new();
    let mut i = 0;
    while i + 3 <= au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            raus.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }
    raus
}

/// Die NALs einer Annex-B-Einheit als Bereiche (Anfang hinter dem Startcode,
/// Ende vor den Nullen des naechsten Startcodes).
fn nal_bereiche(au: &[u8]) -> Vec<(usize, usize)> {
    let codes = startcodes(au);
    let mut raus = Vec::with_capacity(codes.len());
    for (k, &c) in codes.iter().enumerate() {
        let anfang = c + 3;
        let mut ende = codes.get(k + 1).copied().unwrap_or(au.len());
        while ende > anfang && au[ende - 1] == 0 {
            ende -= 1;
        }
        if ende > anfang {
            raus.push((anfang, ende));
        }
    }
    raus
}

/// Das erste SPS-NAL einer Annex-B-Einheit (ohne Startcode) - fuer die
/// Protokollzeile.
pub fn erstes_sps(au: &[u8]) -> Option<&[u8]> {
    nal_bereiche(au).into_iter()
        .map(|(a, e)| &au[a..e])
        .find(|nal| nal[0] & 0x1f == NAL_SPS)
}

/// Eine Annex-B-Zugriffseinheit, in der jedes SPS-NAL durch die Fassung mit
/// VUI ersetzt ist. Alles andere (PPS, Scheiben, Startcodes) bleibt Byte
/// fuer Byte. None, wenn nichts umzuschreiben war - kein SPS, oder eines,
/// das `h264_sps_mit_vui` liegen laesst; dann bleibt die Einheit ohne Kopie.
pub fn h264_au_mit_vui(au: &[u8]) -> Option<Vec<u8>> {
    let bereiche = nal_bereiche(au);
    let mut ersatz: Vec<((usize, usize), Vec<u8>)> = Vec::new();
    for &(a, e) in &bereiche {
        if au[a] & 0x1f == NAL_SPS {
            if let Some(neu) = h264_sps_mit_vui(&au[a..e]) {
                ersatz.push(((a, e), neu));
            }
        }
    }
    if ersatz.is_empty() {
        return None;
    }
    let mut raus = Vec::with_capacity(au.len() + ersatz.len() * 8);
    let mut pos = 0;
    for ((a, e), neu) in ersatz {
        raus.extend_from_slice(&au[pos..a]);
        raus.extend_from_slice(&neu);
        pos = e;
    }
    raus.extend_from_slice(&au[pos..]);
    Some(raus)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Das VUI hinter dem Flag, so weit es hier gebraucht wird:
    /// (bitstream_restriction_flag, max_num_reorder_frames,
    /// max_dec_frame_buffering).
    fn vui_lesen(rbsp: &[u8], pos: usize) -> Option<(bool, u32, u32)> {
        let mut l = BitLeser::neu(rbsp);
        l.pos = pos;
        assert!(l.u1()?, "vui_parameters_present_flag");
        assert!(!l.u1()?, "aspect_ratio_info_present_flag");
        assert!(!l.u1()?, "overscan_info_present_flag");
        assert!(!l.u1()?, "video_signal_type_present_flag");
        assert!(!l.u1()?, "chroma_loc_info_present_flag");
        assert!(!l.u1()?, "timing_info_present_flag");
        assert!(!l.u1()?, "nal_hrd_parameters_present_flag");
        assert!(!l.u1()?, "vcl_hrd_parameters_present_flag");
        assert!(!l.u1()?, "pic_struct_present_flag");
        let restriction = l.u1()?;
        assert!(l.u1()?, "motion_vectors_over_pic_boundaries_flag");
        assert_eq!(l.ue()?, 2, "max_bytes_per_pic_denom");
        assert_eq!(l.ue()?, 1, "max_bits_per_mb_denom");
        assert_eq!(l.ue()?, 16, "log2_max_mv_length_horizontal");
        assert_eq!(l.ue()?, 16, "log2_max_mv_length_vertical");
        let reorder = l.ue()?;
        let puffer = l.ue()?;
        // Abschluss: eine Eins, dann nur Nullen bis zum Ende.
        assert_eq!(l.bit()?, 1, "rbsp_stop_one_bit");
        while l.pos < rbsp.len() * 8 {
            assert_eq!(l.bit()?, 0, "rbsp_alignment_zero_bit");
        }
        Some((restriction, reorder, puffer))
    }

    /// Echte SPS aus VideoToolbox auf dem M1, mit den Einstellungen des Hosts
    /// (H.264 High, RealTime, AllowFrameReordering aus, MaxFrameDelayCount 1),
    /// ausgelesen aus der Formatbeschreibung des ersten Bildes:
    /// 1920x1080 - Level 5.1, 1 Referenzbild, 120x68 Makrobloecke, unten
    /// 8 Zeilen beschnitten (frame_crop_bottom_offset 4), KEIN VUI.
    const VT_1080P: [u8; 12] = [0x27, 0x64, 0x00, 0x33, 0xAC, 0x56, 0x80, 0x78, 0x02, 0x27, 0xE5, 0x40];
    /// Dasselbe bei 1280x720 - Level 4.2, 80x45 Makrobloecke, kein Beschnitt.
    const VT_720P: [u8; 10] = [0x27, 0x64, 0x00, 0x2A, 0xAC, 0x56, 0x80, 0x50, 0x05, 0xB9];

    /// Ein kuenstliches SPS aus dem Schreiber (Felder aehnlich VideoToolbox,
    /// 1280x720) - inklusive der drei Flags hinter frame_mbs_only_flag
    /// (direct_8x8_inference_flag, frame_cropping_flag,
    /// vui_parameters_present_flag) und dem Abschluss. Damit lassen sich die
    /// Faelle bauen, die VideoToolbox nicht liefert (mit VUI, mit Beschnitt).
    fn test_sps(vui: bool, cropping: bool) -> Vec<u8> {
        let mut s = BitSchreiber::neu();
        s.u(8, 100); // profile_idc
        s.u(8, 0); // constraint_set-Flags
        s.u(8, 42); // level_idc
        s.ue(0); // seq_parameter_set_id
        s.ue(1); // chroma_format_idc
        s.ue(0); // bit_depth_luma_minus8
        s.ue(0); // bit_depth_chroma_minus8
        s.u1(false); // qpprime_y_zero_transform_bypass_flag
        s.u1(false); // seq_scaling_matrix_present_flag
        s.ue(4); // log2_max_frame_num_minus4
        s.ue(0); // pic_order_cnt_type
        s.ue(4); // log2_max_pic_order_cnt_lsb_minus4
        s.ue(1); // max_num_ref_frames
        s.u1(false); // gaps_in_frame_num_value_allowed_flag
        s.ue(79); // pic_width_in_mbs_minus1 (1280)
        s.ue(44); // pic_height_in_map_units_minus1 (720)
        s.u1(true); // frame_mbs_only_flag
        s.u1(true); // direct_8x8_inference_flag
        s.u1(cropping); // frame_cropping_flag
        if cropping {
            s.ue(0);
            s.ue(0);
            s.ue(0);
            s.ue(4);
        }
        s.u1(vui); // vui_parameters_present_flag
        if vui {
            // Ein karges VUI: nur timing_info, wie es manche Encoder tun.
            for _ in 0..4 {
                s.u1(false);
            }
            s.u1(true); // timing_info_present_flag
            s.u(32, 1);
            s.u(32, 120);
            s.u1(true); // fixed_frame_rate_flag
            for _ in 0..4 {
                s.u1(false);
            }
        }
        let mut nal = vec![0x67];
        nal.extend_from_slice(&schuetzen(&s.abschluss()));
        nal
    }

    #[test]
    fn exp_golomb_rundlauf() {
        let mut s = BitSchreiber::neu();
        let werte = [0u32, 1, 2, 3, 7, 8, 15, 16, 79, 255, 1000, 65535];
        for &w in &werte {
            s.ue(w);
        }
        let se_werte = [0i32, 1, -1, 2, -2, 17, -17, 1000];
        for &w in &se_werte {
            s.se(w);
        }
        let bytes = s.abschluss();
        let mut l = BitLeser::neu(&bytes);
        for &w in &werte {
            assert_eq!(l.ue(), Some(w));
        }
        for &w in &se_werte {
            assert_eq!(l.se(), Some(w));
        }
        assert_eq!(l.bit(), Some(1));
    }

    #[test]
    fn echte_videotoolbox_sps() {
        // (SPS, Makrobloecke breit, Karteneinheiten hoch, Beschnitt, Level,
        // Bitposition des VUI-Flags in der RBSP)
        let faelle: [(&[u8], u32, u32, Option<[u32; 4]>, u32, usize); 2] = [
            (&VT_1080P, 120, 68, Some([0, 0, 0, 4]), 51, 80),
            (&VT_720P, 80, 45, None, 42, 70),
        ];
        for (alt, breite, hoehe, beschnitt, level, flag) in faelle {
            let alt_rbsp = entschuetzen(&alt[1..]);
            let a = sps_lesen(&alt_rbsp).expect("VideoToolbox-SPS geht auf");
            assert_eq!(a.profile_idc, 100);
            assert_eq!(a.level_idc, level);
            assert_eq!(a.max_num_ref_frames, 1);
            assert_eq!(a.breite_mb, breite);
            assert_eq!(a.hoehe_einheiten, hoehe);
            assert_eq!(a.frame_cropping, beschnitt);
            assert_eq!(a.vui_flag_pos, flag);
            assert!(!a.vui_present, "VideoToolbox schreibt kein VUI - deshalb der ganze Umbau");

            let neu = h264_sps_mit_vui(alt).expect("wird umgeschrieben");
            // Kopfbyte unveraendert (nal_ref_idc 1, Typ 7 - nicht 0x67).
            assert_eq!(neu[0], 0x27);
            let rbsp = entschuetzen(&neu[1..]);
            let n = sps_lesen(&rbsp).unwrap();
            assert!(n.vui_present);
            assert_eq!(
                (n.profile_idc, n.level_idc, n.max_num_ref_frames, n.breite_mb, n.hoehe_einheiten, n.frame_cropping, n.vui_flag_pos),
                (a.profile_idc, a.level_idc, a.max_num_ref_frames, a.breite_mb, a.hoehe_einheiten, a.frame_cropping, a.vui_flag_pos)
            );
            // Alles vor dem Flag ist bitgenau das alte.
            let mut la = BitLeser::neu(&alt_rbsp);
            let mut ln = BitLeser::neu(&rbsp);
            for _ in 0..flag {
                assert_eq!(la.bit(), ln.bit());
            }
            let (restriction, reorder, puffer) = vui_lesen(&rbsp, flag).unwrap();
            assert!(restriction);
            assert_eq!(reorder, 0);
            assert_eq!(puffer, 1);
        }
        // Die Laengen, die das Protokoll bei 1080p meldet ("12 -> 16 Byte"):
        // 80 Bit bis zum Flag, 39 Bit VUI, Abschlussbit = 120 Bit, plus Kopf.
        assert_eq!(h264_sps_mit_vui(&VT_1080P).unwrap().len(), 16);
        assert_eq!(h264_sps_mit_vui(&VT_720P).unwrap().len(), 15);
    }

    #[test]
    fn abgeschnittenes_sps_bleibt_liegen() {
        // Fehlen hinter frame_mbs_only_flag die letzten Flags, geht das SPS
        // nicht auf - nichts umschreiben, was man nicht bis zum Ende gelesen
        // hat.
        let kurz = &VT_720P[..VT_720P.len() - 1];
        assert_eq!(h264_sps_mit_vui(kurz), None);
    }

    #[test]
    fn sps_bekommt_vui() {
        for cropping in [false, true] {
            let alt = test_sps(false, cropping);
            let alt_felder = sps_lesen(&entschuetzen(&alt[1..])).unwrap();
            assert!(!alt_felder.vui_present);
            let neu = h264_sps_mit_vui(&alt).unwrap();
            assert_eq!(neu[0], 0x67);
            assert!(neu.len() > alt.len());
            let rbsp = entschuetzen(&neu[1..]);
            let felder = sps_lesen(&rbsp).unwrap();
            assert!(felder.vui_present);
            assert_eq!(felder.profile_idc, 100);
            assert_eq!(felder.level_idc, 42);
            assert_eq!(felder.max_num_ref_frames, 1);
            assert_eq!(felder.breite_mb, 80);
            assert_eq!(felder.hoehe_einheiten, 45);
            assert_eq!(felder.frame_cropping, if cropping { Some([0, 0, 0, 4]) } else { None });
            assert_eq!(felder.vui_flag_pos, alt_felder.vui_flag_pos);
            // Alles vor dem Flag ist bitgenau das alte.
            let alt_rbsp = entschuetzen(&alt[1..]);
            let mut a = BitLeser::neu(&alt_rbsp);
            let mut n = BitLeser::neu(&rbsp);
            for _ in 0..felder.vui_flag_pos {
                assert_eq!(a.bit(), n.bit());
            }
            let (restriction, reorder, puffer) = vui_lesen(&rbsp, felder.vui_flag_pos).unwrap();
            assert!(restriction);
            assert_eq!(reorder, 0);
            assert_eq!(puffer, 1);
        }
    }

    #[test]
    fn sps_mit_vui_bleibt() {
        let alt = test_sps(true, false);
        assert!(sps_lesen(&entschuetzen(&alt[1..])).unwrap().vui_present);
        assert_eq!(h264_sps_mit_vui(&alt), None);
        // Kein SPS: ebenfalls None.
        assert_eq!(h264_sps_mit_vui(&[0x68, 0xEB, 0xE3, 0xCB]), None);
        assert_eq!(h264_sps_mit_vui(&[]), None);
    }

    #[test]
    fn emulationsschutz_rundlauf() {
        // Hin und zurueck ueber alle heiklen Folgen.
        let roh = [0u8, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0xAB];
        let geschuetzt = schuetzen(&roh);
        assert_eq!(geschuetzt, [0u8, 0, 3, 0, 0, 3, 1, 0, 0, 3, 2, 0, 0, 3, 3, 0, 0, 4, 0xAB]);
        assert_eq!(entschuetzen(&geschuetzt), roh);
        // Ein SPS, dessen Nutzlast hinter dem VUI-Anfang 00 00 0x enthaelt:
        // grosse Breite/Hoehe mit vielen fuehrenden Nullen im Exp-Golomb,
        // dazu ein Level 0 - und die Bits des neuen VUI. Egal wie: die
        // geschuetzte Fassung darf nirgends 00 00 00..03 enthalten, und
        // entschuetzt muss das SPS mit denselben Feldern aufgehen.
        let mut s = BitSchreiber::neu();
        s.u(8, 66); // profile_idc (Baseline: keine Chroma-Felder)
        s.u(8, 0);
        s.u(8, 0); // level_idc 0 -> 00 00 im Kopf
        s.ue(1); // seq_parameter_set_id - die Lage macht aus den Nullen
                 // der Hoehe ein 00 00 02 (Startcode-Verwechslung)
        s.ue(0); // log2_max_frame_num_minus4
        s.ue(2); // pic_order_cnt_type 2
        s.ue(0); // max_num_ref_frames 0
        s.u1(false);
        s.ue(4095); // pic_width_in_mbs_minus1: 11 Nullen, 1, 12 Bits
        s.ue(4095);
        s.u1(true);
        s.u1(false);
        s.u1(false);
        s.u1(false); // vui_parameters_present_flag
        let rbsp = s.abschluss();
        let mut alt = vec![0x67];
        alt.extend_from_slice(&schuetzen(&rbsp));
        // Die Rohfassung traegt tatsaechlich Doppelnullen.
        assert!(rbsp.windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 3));
        let neu = h264_sps_mit_vui(&alt).unwrap();
        // Geschuetzt: nie 00 00 00..02, und ein 00 00 03 nur als Schutz
        // (d. h. gefolgt von 00..03).
        assert!(!neu[1..].windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] < 3));
        for (i, w) in neu[1..].windows(3).enumerate() {
            if w == [0, 0, 3] {
                assert!(neu[1..].get(i + 3).map(|&b| b <= 3).unwrap_or(true));
            }
        }
        assert_ne!(entschuetzen(&neu[1..]), neu[1..].to_vec(), "der Schutz greift");
        let felder = sps_lesen(&entschuetzen(&neu[1..])).unwrap();
        assert_eq!(felder.breite_mb, 4096);
        assert_eq!(felder.hoehe_einheiten, 4096);
        assert_eq!(felder.max_num_ref_frames, 0);
        assert!(felder.vui_present);
        let (r, reorder, puffer) = vui_lesen(&entschuetzen(&neu[1..]), felder.vui_flag_pos).unwrap();
        assert!(r);
        assert_eq!(reorder, 0);
        assert_eq!(puffer, 0);
    }

    #[test]
    fn au_ersetzt_nur_das_sps() {
        let sps = test_sps(false, false);
        let pps = [0x68u8, 0xEB, 0xE3, 0xCB, 0x22, 0xC0];
        let idr = [0x65u8, 0x88, 0x84, 0x00, 0x33, 0xFF];
        let mut au = vec![0, 0, 0, 1];
        au.extend_from_slice(&sps);
        au.extend_from_slice(&[0, 0, 0, 1]);
        au.extend_from_slice(&pps);
        au.extend_from_slice(&[0, 0, 1]);
        au.extend_from_slice(&idr);
        let neu = h264_au_mit_vui(&au).unwrap();
        let sps_neu = h264_sps_mit_vui(&sps).unwrap();
        let mut erwartet = vec![0, 0, 0, 1];
        erwartet.extend_from_slice(&sps_neu);
        erwartet.extend_from_slice(&[0, 0, 0, 1]);
        erwartet.extend_from_slice(&pps);
        erwartet.extend_from_slice(&[0, 0, 1]);
        erwartet.extend_from_slice(&idr);
        assert_eq!(neu, erwartet);
        assert_eq!(erstes_sps(&au), Some(&sps[..]));
        assert_eq!(erstes_sps(&neu), Some(&sps_neu[..]));
        // Ohne SPS - oder mit einem, das schon ein VUI hat - keine Kopie.
        let mut ohne = vec![0, 0, 0, 1];
        ohne.extend_from_slice(&pps);
        ohne.extend_from_slice(&[0, 0, 0, 1]);
        ohne.extend_from_slice(&idr);
        assert_eq!(h264_au_mit_vui(&ohne), None);
        assert_eq!(erstes_sps(&ohne), None);
        let mut mit = vec![0, 0, 0, 1];
        mit.extend_from_slice(&test_sps(true, false));
        mit.extend_from_slice(&[0, 0, 0, 1]);
        mit.extend_from_slice(&idr);
        assert_eq!(h264_au_mit_vui(&mit), None);
        assert_eq!(h264_au_mit_vui(&[]), None);
    }
}
