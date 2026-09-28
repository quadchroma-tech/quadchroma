// Gemeinsame Teile der Goldbildtests beider Anzeigen (--anzeigetest):
// Direct3D 11 unter Windows (anzeige.rs) und Metal auf dem Mac
// (anzeige_mac.rs). Beide vergleichen den Weg ueber die Karte mit dem
// CPU-Weg (`to_rgb`, `blit`, Canvas) und legen dieselbe Oberflaeche
// darueber.

use crate::ui;

/// Der Wert einer Ebene bei (x, y) im Probebild (bw x bh ist die Groesse
/// DIESER Ebene): Verlaeufe in allen drei Ebenen, ein Streifen harter Kanten
/// (Schachbrett in ungeraden Feldgroessen, damit die Felder nicht auf den
/// 2x2-Farbraster fallen) und in den Ecken die Extremwerte 0 und `max`, je
/// Ebene (0 = Y, 1 = U, 2 = V) anders verteilt, damit das Abschneiden auf
/// 0..255 in jeder Richtung vorkommt.
pub fn probewert(x: usize, y: usize, bw: usize, bh: usize, ebene: usize, max: u32) -> u32 {
    let ecke = 8.min(bw / 4).min(bh / 4);
    let (links, rechts) = (x < ecke, x + ecke >= bw);
    let (oben, unten) = (y < ecke, y + ecke >= bh);
    if (links || rechts) && (oben || unten) {
        let n = links as usize + 2 * oben as usize + ebene;
        return if n % 2 == 0 { 0 } else { max };
    }
    if y >= bh / 3 && y < bh / 3 + 16.min(bh / 4) {
        let k = 5 + 2 * ebene;
        return if ((x / k) + (y / 3)) % 2 == 0 { 0 } else { max };
    }
    let (bw1, bh1) = ((bw - 1).max(1) as u64, (bh - 1).max(1) as u64);
    (match ebene {
        0 => x as u64 * max as u64 / bw1,
        1 => y as u64 * max as u64 / bh1,
        _ => (x * 3 + y * 5) as u64 * max as u64 / (3 * bw1 + 5 * bh1),
    }) as u32
}

/// Wie `probewert`, fuer die PQ-Probebilder beider Anzeigen: das untere
/// Viertel ist ein neutraler Streifen (Y ueber alle Codes, Cb = Cr = Mitte) -
/// dort wirkt die Tonkurve selbst, darueber auch Farben weit ausserhalb
/// jedes Farbraums und Pegel bis 10000 nit. Die Mitte ist die des Formats,
/// auch oben buendig: 128, 512, 32768 (fuer 65472, 65520 und 65535).
pub fn probewert_pq(x: usize, y: usize, bw: usize, bh: usize, ebene: usize, max: u32) -> u32 {
    if y < bh * 3 / 4 {
        return probewert(x, y, bw, bh, ebene, max);
    }
    if ebene == 0 {
        (x as u64 * max as u64 / (bw - 1).max(1) as u64) as u32
    } else {
        (max + 1).next_power_of_two() / 2
    }
}

/// Die Oberflaeche des Tests: alles, was die echte zeichnet - Schleier wie
/// im Menue (auf der oberen Haelfte, damit die andere ohne Oberflaeche
/// bleibt und der Kasten nicht das ganze Ziel ist), Tafel, Schrift in drei
/// Groessen, Neonlinie, Knopf, Schalter, Verlauf, Eingabefeld, halbdurchsichtige
/// Flaeche.
pub fn oberflaeche_probe(c: &mut ui::Canvas, u: &mut ui::Ui, zeile: &str) {
    let (w, h) = (c.w as i32, c.h as i32);
    c.fill(0, 0, w, h / 2, ui::BG, 205);
    c.panel(40, 60, 300, 130, ui::CYAN);
    u.text.draw(c, 56, 92, "QuadChroma Anzeigetest", 15, ui::TEXT, 2);
    // Format und Groesse im Text: so traegt der Kasten-Upload je Durchlauf
    // wirklich neuen Inhalt in eine Textur, die es schon gibt.
    u.text.draw(c, 56, 116, zeile, 13, ui::DIM, 1);
    u.text.draw_centered(c, 190, 170, "628 306", 30, ui::AMBER, 6);
    c.glow_hline(40, 210, 300, ui::MAGENTA);
    c.glow_vline(360, 60, 150, ui::CYAN);
    let _ = u.button(c, ui::Rect { x: 400, y: 80, w: 140, h: 36 }, "Verbinden", ui::CYAN);
    let _ = u.toggle(c, ui::Rect { x: 400, y: 140, w: 200, h: 28 }, "Ton", true);
    let verlauf: Vec<f32> = (0..120).map(|i| 110.0 + 15.0 * ((i as f32) / 9.0).sin()).collect();
    u.spark_range(c, ui::Rect { x: 400, y: 200, w: 200, h: 50 }, &verlauf, 90.0, 130.0, ui::CYAN);
    c.rect(60, 300, 200, 60, ui::AMBER, 90);
    u.field(c, ui::Rect { x: 400, y: 300, w: 240, h: 34 }, "192.168.178.194", "Adresse", true);
}

/// Ein Vergleich: groesste und mittlere Abweichung ueber alle Kanaele, eine
/// Zeile, und bei jeder Abweichung ein Differenzbild (Abweichung x 64, damit
/// eine Stufe sichtbar wird); bei Ueberschreitung dazu beide Bilder.
#[allow(clippy::too_many_arguments)]
pub fn fall_pruefen(
    name: &str, w: u32, h: u32, fall: &str,
    cpu: &[u32], karte: &[u32], bw: u32, bh: u32, toleranz: u32,
    verzeichnis: Option<&str>, lang: &'static crate::strings::Lang,
) -> bool {
    let n = (bw as usize) * (bh as usize);
    if cpu.len() < n || karte.len() < n {
        println!("{name:<15} {w:>4}x{h:<4} {fall:<11} FEHLER: Puffer zu klein ({} / {} statt {n})", cpu.len(), karte.len());
        return false;
    }
    let mut max = 0u32;
    let mut summe = 0u64;
    let mut diff = vec![0u32; n];
    for i in 0..n {
        let (a, b) = (cpu[i], karte[i]);
        let d = [16, 8, 0].iter().map(|s| ((a >> s) & 255).abs_diff((b >> s) & 255)).max().unwrap_or(0);
        max = max.max(d);
        summe += d as u64;
        let g = (d * 64).min(255);
        diff[i] = (g << 16) | (g << 8) | g;
    }
    let mittel = summe as f64 / n as f64;
    let ok = max <= toleranz;
    println!(
        "{name:<15} {w:>4}x{h:<4} {fall:<11} max {max:>3}  mittel {mittel:.4}  {}",
        if ok { "ok".to_string() } else { format!("FEHLER (Toleranz {toleranz})") }
    );
    if let (true, Some(verzeichnis)) = (max > 0, verzeichnis) {
        let basis = std::path::Path::new(verzeichnis).join(format!("{}-{w}x{h}-{fall}", name.to_lowercase()));
        let basis = basis.to_string_lossy();
        crate::write_bmp(&format!("{basis}-diff.bmp"), bw as usize, bh as usize, &diff, lang);
        if !ok {
            crate::write_bmp(&format!("{basis}-cpu.bmp"), bw as usize, bh as usize, cpu, lang);
            crate::write_bmp(&format!("{basis}-gpu.bmp"), bw as usize, bh as usize, karte, lang);
        }
    }
    ok
}
