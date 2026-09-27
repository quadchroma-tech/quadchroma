// Bauskript des Clients, je Ziel ein Zweig:
//
// Windows: bettet Versionsinfo, Programmsymbol und Anwendungsmanifest in die
// exe ein (res/quadchroma.rc, res/quadchroma.manifest, res/quadchroma.ico).
//
// macOS: baut die Host-Engine des Mac (host/, Objective-C und C) als
// libqchost.a hinein - dieselben Quellen und Schalter wie QuadChroma.app,
// aus dem Makefile gelesen, ohne host/start.m (dort steht das main der
// eigenen App). Siehe host_engine_einbauen und src/host_mac.rs.
//
// Andere Ziele: nichts.
//
// ------------------------------------------------------------------ Windows
//
// Ohne zusaetzliche Crates: der Ressourcen-Compiler des Windows SDK (rc.exe)
// oder llvm-rc uebersetzt die .rc zu einer .res-Datei, und die geht als
// Linker-Argument in jede Binaerdatei des Pakets (cargo::rustc-link-arg-bins,
// also auch in die Test-exe). Versionsnummer und Zielarchitektur kommen aus
// Cargo.toml bzw. vom Ziel; build.rs schreibt sie in eine Kopfdatei fuer die
// .rc und fuellt die Manifest-Vorlage (@VERSION@, @ARCH@).
//
// Fehlt ein Ressourcen-Compiler, geht der Bau mit einer Warnung ohne
// Ressourcen weiter: die exe laeuft auch so, nur Versionsinfo, Symbol und
// Manifest fehlen. Fuer Releases setzt man QC_RESSOURCEN_PFLICHT=1, dann
// bricht der Bau in dem Fall ab. Scheitert ein gefundener Compiler an der
// .rc, bricht der Bau immer ab - dann ist eine Datei in res/ kaputt.
//
// Cargo merkt sich das Ergebnis der Suche: build.rs laeuft erst wieder, wenn
// sich build.rs, Cargo.toml, res/* oder eine der Variablen unten aendert
// (dazu WindowsSdkVerBinPath, das die Entwickler-Eingabeaufforderung setzt).
// Wer das SDK oder LLVM nachinstalliert, stoesst den Lauf einmal mit
// `cargo clean -p quadchroma` an - sonst bleibt die exe ohne Ressourcen.
//
// Umgebungsvariablen:
//   QC_RC=<pfad>             diesen Ressourcen-Compiler nehmen (rc.exe oder llvm-rc.exe)
//   QC_RESSOURCEN_PFLICHT=1  ohne Ressourcen-Compiler abbrechen statt warnen (Releases, CI)
//   QC_OHNE_RESSOURCEN=1     ohne Ressourcen bauen (nur zum Ausprobieren)
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => windows_ressourcen(),
        Ok("macos") => {
            if let Err(e) = host_engine_einbauen() {
                panic!("Host-Engine (host/): {e}");
            }
        }
        _ => {}
    }
}

fn windows_ressourcen() {
    // Nur diese Eingaben loesen einen neuen Lauf aus (sonst jede Datei im Paket).
    for p in ["build.rs", "Cargo.toml", "res/quadchroma.rc", "res/quadchroma.manifest", "res/quadchroma.ico"] {
        println!("cargo::rerun-if-changed={p}");
    }
    // WindowsSdkVerBinPath: Wechsel in eine bzw. aus einer Entwickler-
    // Eingabeaufforderung wiederholt die Suche nach rc.exe. PATH absichtlich
    // nicht (sonst Neulauf und Relink bei jedem Shell-Wechsel).
    for v in ["QC_RC", "QC_RESSOURCEN_PFLICHT", "QC_OHNE_RESSOURCEN", "WindowsSdkVerBinPath"] {
        println!("cargo::rerun-if-env-changed={v}");
    }
    if gesetzt("QC_OHNE_RESSOURCEN") {
        println!("cargo::warning=QC_OHNE_RESSOURCEN gesetzt: exe ohne Versionsinfo, Symbol und Manifest");
        return;
    }
    match einbetten() {
        Ok(()) => {}
        Err(Fehler::KeinCompiler(gesucht)) if !gesetzt("QC_RESSOURCEN_PFLICHT") => {
            println!("cargo::warning=kein Ressourcen-Compiler gefunden (rc.exe aus dem Windows SDK oder llvm-rc.exe): exe ohne Versionsinfo, Symbol und Manifest");
            for g in gesucht {
                println!("cargo::warning=  gesucht: {g}");
            }
            println!("cargo::warning=Abhilfe: Windows SDK (Visual Studio Build Tools) oder LLVM installieren oder QC_RC=<Pfad zu rc.exe/llvm-rc.exe> setzen");
            println!("cargo::warning=nach der Installation: cargo clean -p quadchroma (oder build.rs anfassen), sonst bleibt die exe ohne Ressourcen - cargo laesst build.rs nur bei Aenderungen an build.rs, Cargo.toml, res/ oder QC_RC, QC_RESSOURCEN_PFLICHT, QC_OHNE_RESSOURCEN, WindowsSdkVerBinPath neu laufen");
        }
        Err(e) => panic!("Windows-Ressourcen: {}", e),
    }
}

/// Ist die Umgebungsvariable gesetzt und nicht leer?
fn gesetzt(name: &str) -> bool {
    env::var_os(name).is_some_and(|v| !v.is_empty())
}

/// Was beim Einbetten schiefgehen kann. Nur ein fehlender Compiler ist
/// "weich" (Warnung, Bau geht weiter); alles andere bricht den Bau ab.
enum Fehler {
    /// Kein rc.exe/llvm-rc gefunden - die Liste der geprueften Orte.
    KeinCompiler(Vec<String>),
    /// Datei fehlt, QC_RC zeigt ins Leere, Compiler scheitert.
    Sonst(String),
}

impl From<String> for Fehler {
    fn from(s: String) -> Self {
        Fehler::Sonst(s)
    }
}

impl std::fmt::Display for Fehler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fehler::KeinCompiler(gesucht) => write!(
                f,
                "kein Ressourcen-Compiler gefunden (rc.exe aus dem Windows SDK oder llvm-rc.exe). Gesucht:\n  {}\n\
                 Abhilfe: Windows SDK (Bestandteil der Visual Studio Build Tools) oder LLVM installieren, \
                 QC_RC=<Pfad zu rc.exe oder llvm-rc.exe> setzen, oder QC_RESSOURCEN_PFLICHT nicht setzen \
                 (exe dann ohne Versionsinfo, Symbol und Manifest).",
                gesucht.join("\n  ")
            ),
            Fehler::Sonst(s) => f.write_str(s),
        }
    }
}

fn einbetten() -> Result<(), Fehler> {
    let wurzel = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    let out = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let res = wurzel.join("res");

    // Version: "0.1.0" -> 0,1,0,0 (vier 16-Bit-Zahlen fuer FILEVERSION) und Text.
    let zahl = |name: &str| -> Result<u16, String> {
        let v = env::var(name).map_err(|_| format!("{name} fehlt"))?;
        v.parse::<u16>().map_err(|_| format!("{name}={v}: keine Zahl 0..65535"))
    };
    let (major, minor, patch) = (zahl("CARGO_PKG_VERSION_MAJOR")?, zahl("CARGO_PKG_VERSION_MINOR")?, zahl("CARGO_PKG_VERSION_PATCH")?);
    let version_text = env::var("CARGO_PKG_VERSION").map_err(|e| e.to_string())?;
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86_64") => "amd64",
        Ok("aarch64") => "arm64",
        Ok("x86") => "x86",
        _ => "*",
    };

    // Manifest: Vorlage fuellen, nach OUT_DIR schreiben.
    let vorlage = std::fs::read_to_string(res.join("quadchroma.manifest")).map_err(|e| format!("res/quadchroma.manifest: {e}"))?;
    let manifest = vorlage.replace("@VERSION@", &format!("{major}.{minor}.{patch}.0")).replace("@ARCH@", arch);
    let manifest_pfad = out.join("quadchroma.manifest");
    std::fs::write(&manifest_pfad, manifest).map_err(|e| format!("{}: {e}", manifest_pfad.display()))?;

    // Kopfdatei fuer die .rc: Zahlen, Text, Pfad des gefuellten Manifests.
    let kopf = format!(
        "#define QC_VERSION_ZAHLEN {major},{minor},{patch},0\n#define QC_VERSION_TEXT \"{version_text}\"\n#define QC_MANIFEST_PFAD \"{}\"\n",
        manifest_pfad.to_string_lossy().replace('\\', "\\\\")
    );
    std::fs::write(out.join("quadchroma_version.h"), kopf).map_err(|e| e.to_string())?;

    // Uebersetzen. Arbeitsordner res/, damit "quadchroma.ico" gefunden wird;
    // OUT_DIR ueber /I fuer die Kopfdatei.
    let (rc, ist_llvm) = ressourcen_compiler()?;
    println!("Ressourcen-Compiler: {}", rc.display());
    let res_pfad = out.join("quadchroma.res");
    let mut cmd = Command::new(&rc);
    cmd.current_dir(&res);
    if !ist_llvm {
        cmd.arg("/nologo");
    }
    // Quelltext ist UTF-8 (LegalCopyright mit "©"): rc.exe liest das
    // #pragma code_page(65001), llvm-rc nicht - deshalb /C 65001 fuer beide.
    cmd.arg("/C").arg("65001");
    cmd.arg("/I").arg(&out).arg("/fo").arg(&res_pfad).arg(res.join("quadchroma.rc"));
    let ausgabe = cmd.output().map_err(|e| format!("{} nicht ausfuehrbar: {e}", rc.display()))?;
    if !ausgabe.status.success() {
        return Err(Fehler::Sonst(format!(
            "{} scheiterte ({}):\n{}{}",
            rc.display(),
            ausgabe.status,
            String::from_utf8_lossy(&ausgabe.stdout),
            String::from_utf8_lossy(&ausgabe.stderr)
        )));
    }
    println!("cargo::rustc-link-arg-bins={}", res_pfad.display());
    Ok(())
}

/// Den Ressourcen-Compiler finden: QC_RC, dann rc.exe aus einer
/// Entwickler-Eingabeaufforderung (WindowsSdkVerBinPath), dann das neueste
/// Windows SDK unter "Windows Kits", dann llvm-rc (LLVM-Installation, PATH).
/// Liefert den Pfad und ob es llvm-rc ist.
fn ressourcen_compiler() -> Result<(PathBuf, bool), Fehler> {
    let mut gesucht: Vec<String> = Vec::new();
    if let Some(p) = env::var_os("QC_RC").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(p);
        if p.is_file() {
            let llvm = ist_llvm(&p);
            return Ok((p, llvm));
        }
        // Ausdruecklich verlangt und nicht da: das ist ein harter Fehler.
        return Err(Fehler::Sonst(format!("QC_RC={} ist keine Datei", p.display())));
    }
    // Werkzeuge des SDK liegen je Host-Architektur in einem Unterordner.
    let host = env::var("HOST").unwrap_or_default();
    let host_dir = if host.starts_with("aarch64") {
        "arm64"
    } else if host.starts_with("i686") {
        "x86"
    } else {
        "x64"
    };
    if let Some(p) = env::var_os("WindowsSdkVerBinPath") {
        let p = PathBuf::from(p).join(host_dir).join("rc.exe");
        if p.is_file() {
            return Ok((p, false));
        }
        gesucht.push(p.display().to_string());
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Some(pf) = env::var_os(var) else { continue };
        let bin = PathBuf::from(pf).join("Windows Kits").join("10").join("bin");
        let Ok(eintraege) = std::fs::read_dir(&bin) else {
            gesucht.push(format!("{} (kein Windows SDK)", bin.display()));
            continue;
        };
        let mut versionen: Vec<PathBuf> = eintraege
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("10.")))
            .collect();
        versionen.sort_by_key(|p| std::cmp::Reverse(versionsschluessel(p)));
        for v in versionen {
            let p = v.join(host_dir).join("rc.exe");
            if p.is_file() {
                return Ok((p, false));
            }
            gesucht.push(p.display().to_string());
        }
    }
    if let Some(pf) = env::var_os("ProgramFiles") {
        let p = PathBuf::from(pf).join("LLVM").join("bin").join("llvm-rc.exe");
        if p.is_file() {
            return Ok((p, true));
        }
        gesucht.push(p.display().to_string());
    }
    for name in ["rc.exe", "llvm-rc.exe"] {
        if let Some(p) = im_pfad(name) {
            let llvm = ist_llvm(&p);
            return Ok((p, llvm));
        }
        gesucht.push(format!("{name} (im PATH)"));
    }
    Err(Fehler::KeinCompiler(gesucht))
}

fn ist_llvm(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase().starts_with("llvm-rc"))
}

fn versionsschluessel(p: &Path) -> Vec<u32> {
    p.file_name().and_then(|n| n.to_str()).unwrap_or("").split('.').map(|t| t.parse().unwrap_or(0)).collect()
}

fn im_pfad(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|p| env::split_paths(&p).map(|d| d.join(name)).find(|f| f.is_file()))
}

// -------------------------------------------------------------------- macOS
//
// Die Host-Engine kommt Datei fuer Datei durch clang (-c, Schalter FLAGS aus
// dem Makefile, dazu -arch des Ziels) nach OUT_DIR/qchost, libtool -static
// packt die Objekte in libqchost.a. Die geht mit -Wl,-force_load in jede
// Binaerdatei des Pakets (auch in die Test-exe): ohne force_load holte der
// Linker nur die Objekte, deren Symbole Rust selbst braucht - menue.o etwa
// kaeme nie hinein, und es blieben die schwachen Standardfassungen
// (qc_oberflaeche_starten in main.m, qc_ui_* in zugang.c), also ein Host ohne
// Menueleiste. Was danach niemand erreicht, entfernt -dead_strip (von rustc
// gesetzt) wie bisher; Objective-C-Klassen bleiben. Die Frameworks stehen
// ebenfalls im Makefile (FRAMEWORKS).
//
// Quellenliste und Schalter werden gelesen, nicht abgeschrieben: was die
// eigene App baut, baut auch der Client. Ohne weitere Crates (kein cc):
// clang und libtool aus den Xcode Command Line Tools.
//
// Neu gebaut wird, wenn sich build.rs, das Makefile oder etwas unter host/
// aendert (cargo durchsucht den Ordner).

/// Die Wurzel des Repositorys (eine Ebene ueber client/).
fn repo_wurzel() -> Result<PathBuf, String> {
    let paket = PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|e| e.to_string())?);
    paket.parent().map(Path::to_path_buf).ok_or_else(|| "CARGO_MANIFEST_DIR ohne Elternordner".to_string())
}

/// Der Wert einer Variablen `name := ...` aus dem Makefile, in Woerter
/// zerlegt. Fortsetzungszeilen (`\` am Ende) gelten als eine Zeile, ein `#`
/// beginnt einen Kommentar. Verweise auf andere Variablen (`$(...)`) liest
/// dieses Skript nicht - dann bricht es mit Hinweis ab, statt falsch zu bauen.
fn make_variable(makefile: &str, name: &str) -> Result<Vec<String>, String> {
    let mut zeile = String::new();
    for roh in makefile.lines() {
        if let Some(vorn) = roh.strip_suffix('\\') {
            zeile.push_str(vorn);
            zeile.push(' ');
            continue;
        }
        zeile.push_str(roh);
        let ganz = std::mem::take(&mut zeile);
        let ohne_kommentar = ganz.split('#').next().unwrap_or("");
        let Some(rest) = ohne_kommentar.strip_prefix(name) else { continue };
        if !rest.starts_with([' ', '\t', ':']) {
            continue; // etwa SRCX := ...
        }
        let Some(wert) = rest.trim_start().strip_prefix(":=") else { continue };
        if wert.contains("$(") {
            return Err(format!("{name} im Makefile verweist auf eine andere Variable - build.rs liest nur feste Werte"));
        }
        return Ok(wert.split_whitespace().map(str::to_string).collect());
    }
    Err(format!("keine Zeile \"{name} := ...\" im Makefile"))
}

fn host_engine_einbauen() -> Result<(), String> {
    for p in ["build.rs", "../Makefile", "../host"] {
        println!("cargo::rerun-if-changed={p}");
    }
    println!("cargo::rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    let wurzel = repo_wurzel()?;
    let out = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let makefile = std::fs::read_to_string(wurzel.join("Makefile")).map_err(|e| format!("Makefile: {e}"))?;

    // start.m traegt das main der eigenen App; im Client ist main das von Rust.
    let quellen: Vec<String> = make_variable(&makefile, "SRC")?.into_iter().filter(|q| q != "host/start.m").collect();
    let schalter = make_variable(&makefile, "FLAGS")?;
    let frameworks = make_variable(&makefile, "FRAMEWORKS")?;
    if quellen.is_empty() {
        return Err("SRC im Makefile ist leer".into());
    }

    // Das Mindestsystem der Engine (-mmacosx-version-min im Makefile) muss
    // auch das des Clients sein, sonst gaebe die exe vor, auf aelteren
    // Systemen zu laufen (client/.cargo/config.toml setzt es).
    if let Some(min) = schalter.iter().find_map(|s| s.strip_prefix("-mmacosx-version-min=")) {
        let ziel = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default();
        if versionsschluessel_text(&ziel) < versionsschluessel_text(min) {
            println!(
                "cargo::warning=MACOSX_DEPLOYMENT_TARGET={} liegt unter dem Mindestsystem der Host-Engine ({min}) - der Linker warnt, und die exe liefe auf aelteren Systemen nicht",
                if ziel.is_empty() { "(nicht gesetzt)" } else { &ziel }
            );
        }
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        Ok(a) => return Err(format!("Zielarchitektur {a} kennt build.rs nicht")),
        Err(e) => return Err(e.to_string()),
    };

    let obj_dir = out.join("qchost");
    std::fs::create_dir_all(&obj_dir).map_err(|e| format!("{}: {e}", obj_dir.display()))?;
    let mut objekte: Vec<PathBuf> = Vec::new();
    for q in &quellen {
        let stamm = Path::new(q).file_stem().and_then(|s| s.to_str()).ok_or_else(|| format!("Quelle ohne Namen: {q}"))?;
        let obj = obj_dir.join(format!("{stamm}.o"));
        if objekte.contains(&obj) {
            return Err(format!("zwei Quellen mit dem Namen {stamm} - die Objekte ueberschrieben sich"));
        }
        objekte.push(obj);
    }

    // Uebersetzen, hoechstens NUM_JOBS clang zugleich (cargo -j).
    let zugleich = env::var("NUM_JOBS").ok().and_then(|n| n.parse::<usize>().ok()).unwrap_or(4).max(1);
    let auftraege: Vec<(&String, &PathBuf)> = quellen.iter().zip(objekte.iter()).collect();
    for gruppe in auftraege.chunks(zugleich) {
        let mut laufend = Vec::new();
        for (q, obj) in gruppe {
            let kind = Command::new("clang")
                .current_dir(&wurzel)
                .args(&schalter)
                .args(["-arch", arch, "-c"])
                .arg(q.as_str())
                .arg("-o")
                .arg(obj)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("clang nicht startbar ({e}) - Xcode Command Line Tools installiert? (xcode-select --install)"))?;
            laufend.push((q, kind));
        }
        for (q, kind) in laufend {
            let aus = kind.wait_with_output().map_err(|e| format!("clang {q}: {e}"))?;
            let fehlertext = String::from_utf8_lossy(&aus.stderr);
            if !aus.status.success() {
                return Err(format!("clang scheiterte an {q} ({}):\n{}{fehlertext}", aus.status, String::from_utf8_lossy(&aus.stdout)));
            }
            // -Wall soll still bleiben; was clang doch meldet, zeigt cargo an.
            for z in fehlertext.lines().filter(|z| !z.trim().is_empty()) {
                println!("cargo::warning={z}");
            }
        }
    }

    let bibliothek = out.join("libqchost.a");
    let _ = std::fs::remove_file(&bibliothek);
    let aus = Command::new("libtool")
        .args(["-static", "-no_warning_for_no_symbols", "-o"])
        .arg(&bibliothek)
        .args(&objekte)
        .output()
        .map_err(|e| format!("libtool nicht startbar: {e}"))?;
    if !aus.status.success() {
        return Err(format!("libtool scheiterte ({}):\n{}", aus.status, String::from_utf8_lossy(&aus.stderr)));
    }

    println!("cargo::rustc-link-arg=-Wl,-force_load,{}", bibliothek.display());
    let mut fw = frameworks.iter();
    while let Some(w) = fw.next() {
        match (w.as_str(), fw.next()) {
            ("-framework", Some(name)) => println!("cargo::rustc-link-lib=framework={name}"),
            _ => return Err(format!("FRAMEWORKS im Makefile: \"-framework Name\" erwartet, gefunden \"{w}\"")),
        }
    }
    // Objective-C-Laufzeit (ARC: objc_retain und Verwandte).
    println!("cargo::rustc-link-lib=dylib=objc");
    Ok(())
}

/// "14.0" -> [14, 0, 0] fuer den Vergleich von Systemversionen ("14" wie
/// "14.0.0"); leer -> [0, 0, 0].
fn versionsschluessel_text(v: &str) -> [u32; 3] {
    let mut s = [0u32; 3];
    for (i, t) in v.split('.').take(3).enumerate() {
        s[i] = t.trim().parse().unwrap_or(0);
    }
    s
}
