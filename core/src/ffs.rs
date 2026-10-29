//! Import ustawień z FreeFileSync (`.ffs_gui` / `.ffs_batch`, XML).
//!
//! Mapowanie: każda para folderów (Left/Right) to osobne zadanie Syncron. Wariant synchronizacji
//! (TwoWay / Mirror / Update / Custom), filtry Include/Exclude, rozmiar i wiek, tryb porównania, tolerancja czasu,
//! przesunięcie czasu, dowiązania, wykrywanie przeniesień i polityka usuwania (kosz/wersjonowanie).
//! Zadania są WYŁĄCZONE (`enabled = false`), bez harmonogramu i bez poleceń przed/po.
use crate::model::{Job, Symlinks, Versioning};
use crate::plan::{Compare, CustomAction, CustomRules, Mode};
use std::path::Path;

const MAX_XML_BYTES: u64 = 8 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

// ---------- minimalny parser XML ----------

#[derive(Debug, Default, Clone)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Node> {
        self.children.iter().filter(move |c| c.name == name)
    }
    /// Pierwszy potomek (dowolnie głęboko) o danej nazwie.
    fn find(&self, name: &str) -> Option<&Node> {
        for c in &self.children {
            if c.name == name {
                return Some(c);
            }
            if let Some(x) = c.find(name) {
                return Some(x);
            }
        }
        None
    }
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
    fn txt(&self) -> &str {
        self.text.trim()
    }
    fn child_txt(&self, name: &str) -> Option<&str> {
        self.child(name).map(Node::txt)
    }
}

struct Parser<'a> {
    s: &'a str,
    i: usize,
}

fn decode_entities(t: &str) -> String {
    if !t.contains('&') {
        return t.to_string();
    }
    let mut out = String::with_capacity(t.len());
    let mut rest = t;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        let Some(end) = rest.find(';').filter(|e| *e <= 12) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.i..]
    }
    fn skip_ws(&mut self) {
        let r = self.rest();
        self.i += r.len() - r.trim_start().len();
    }
    /// Pomija białe znaki, deklaracje `<?...?>`, komentarze i DOCTYPE.
    fn skip_misc(&mut self) -> Result<(), String> {
        loop {
            self.skip_ws();
            let r = self.rest();
            if r.starts_with("<?") {
                let e = r
                    .find("?>")
                    .ok_or("Nieprawidłowy XML: niezamknięta deklaracja.")?;
                self.i += e + 2;
            } else if r.starts_with("<!--") {
                let e = r
                    .find("-->")
                    .ok_or("Nieprawidłowy XML: niezamknięty komentarz.")?;
                self.i += e + 3;
            } else if r.starts_with("<!DOCTYPE") || r.starts_with("<!doctype") {
                // DTD (encje zewnętrzne) ignorujemy w całości
                let e = r
                    .find('>')
                    .ok_or("Nieprawidłowy XML: niezamknięty DOCTYPE.")?;
                self.i += e + 1;
            } else {
                return Ok(());
            }
        }
    }
    fn element(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("Nieprawidłowy XML: zbyt głębokie zagnieżdżenie.".into());
        }
        if !self.rest().starts_with('<') {
            return Err("Nieprawidłowy XML: oczekiwano znacznika.".into());
        }
        self.i += 1;
        let r = self.rest();
        let n = r
            .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
            .ok_or("Nieprawidłowy XML: niezamknięty znacznik.")?;
        let mut node = Node {
            name: r[..n].to_string(),
            ..Default::default()
        };
        self.i += n;
        // atrybuty
        loop {
            self.skip_ws();
            let r = self.rest();
            if r.starts_with("/>") {
                self.i += 2;
                return Ok(node);
            }
            if r.starts_with('>') {
                self.i += 1;
                break;
            }
            let eq = r.find('=').ok_or("Nieprawidłowy XML: błąd w atrybutach.")?;
            let key = r[..eq].trim().to_string();
            self.i += eq + 1;
            self.skip_ws();
            let r = self.rest();
            let q = r.chars().next().filter(|c| *c == '"' || *c == '\'');
            let q = q.ok_or("Nieprawidłowy XML: wartość atrybutu bez cudzysłowu.")?;
            let end = r[1..]
                .find(q)
                .ok_or("Nieprawidłowy XML: niezamknięta wartość atrybutu.")?;
            node.attrs.push((key, decode_entities(&r[1..1 + end])));
            self.i += end + 2;
        }
        // zawartość
        loop {
            let r = self.rest();
            if r.is_empty() {
                return Err(format!(
                    "Nieprawidłowy XML: brak zamknięcia </{}>.",
                    node.name
                ));
            }
            if r.starts_with("</") {
                let e = r
                    .find('>')
                    .ok_or("Nieprawidłowy XML: niezamknięty znacznik końcowy.")?;
                let close = r[2..e].trim();
                if close != node.name {
                    return Err(format!(
                        "Nieprawidłowy XML: </{close}> zamiast </{}>.",
                        node.name
                    ));
                }
                self.i += e + 1;
                return Ok(node);
            }
            if r.starts_with("<!--") {
                let e = r
                    .find("-->")
                    .ok_or("Nieprawidłowy XML: niezamknięty komentarz.")?;
                self.i += e + 3;
            } else if let Some(cd) = r.strip_prefix("<![CDATA[") {
                let e = cd
                    .find("]]>")
                    .ok_or("Nieprawidłowy XML: niezamknięta sekcja CDATA.")?;
                node.text.push_str(&cd[..e]);
                self.i += 9 + e + 3;
            } else if r.starts_with("<?") {
                let e = r
                    .find("?>")
                    .ok_or("Nieprawidłowy XML: niezamknięta instrukcja.")?;
                self.i += e + 2;
            } else if r.starts_with('<') {
                let c = self.element(depth + 1)?;
                node.children.push(c);
            } else {
                let e = r.find('<').unwrap_or(r.len());
                node.text.push_str(&decode_entities(&r[..e]));
                self.i += e;
            }
        }
    }
}

fn parse_xml(s: &str) -> Result<Node, String> {
    let mut p = Parser { s, i: 0 };
    p.skip_misc()?;
    let root = p.element(0)?;
    Ok(root)
}

// ---------- mapowanie ----------

fn truthy(s: &str) -> bool {
    matches!(
        s.trim().to_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

/// Maska FreeFileSync -> maski Syncron (glob): `\` to separator, wiodący `\` = od korzenia, końcowy `\` = folder.
/// Maska bez ukośników pasuje do nazwy na dowolnej głębokości; dla folderów dodajemy też zawartość.
fn mask_to_globs(item: &str) -> Vec<String> {
    let t = item.trim().replace('\\', "/");
    let anchored = t.starts_with('/');
    let t = t.trim_start_matches('/');
    let dir = t.ends_with('/');
    let t = t.trim_end_matches('/');
    if t.is_empty() || t == "*" && !dir && !anchored {
        return if t == "*" { vec!["*".into()] } else { vec![] };
    }
    let mut out = Vec::new();
    if !dir {
        out.push(t.to_string());
    }
    out.push(format!("{t}/*"));
    if !anchored {
        out.push(format!("*/{t}/*"));
    }
    out
}

fn collect_masks(n: Option<&Node>) -> Vec<String> {
    let Some(n) = n else { return vec![] };
    let mut raw: Vec<String> = Vec::new();
    let split = |t: &str, raw: &mut Vec<String>| {
        for p in t.split(['\n', '\r', '|', ';']) {
            if !p.trim().is_empty() {
                raw.push(p.trim().to_string());
            }
        }
    };
    split(&n.text, &mut raw);
    for it in &n.children {
        split(&it.text, &mut raw);
    }
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        for g in mask_to_globs(&r) {
            if !out.contains(&g) {
                out.push(g);
            }
        }
    }
    out
}

fn unit_mult(unit: Option<&str>) -> Option<u64> {
    match unit.map(str::to_lowercase).as_deref() {
        Some("byte") | Some("bytes") | Some("b") => Some(1),
        Some("kb") => Some(1024),
        Some("mb") => Some(1024 * 1024),
        Some("gb") => Some(1024 * 1024 * 1024),
        _ => None, // None / brak: filtr wyłączony
    }
}

fn parse_hours(t: &str) -> u32 {
    t.split(|c: char| !c.is_ascii_digit())
        .filter_map(|x| x.parse::<u32>().ok())
        .max()
        .unwrap_or(0)
        .min(23)
}

fn apply_compare(job: &mut Job, n: &Node) {
    if let Some(v) = n.child_txt("Variant") {
        let v = v.to_lowercase();
        job.compare = if v.contains("content") {
            Compare::Content
        } else if v.contains("size") && !v.contains("time") {
            Compare::SizeOnly
        } else {
            Compare::TimeAndSize
        };
    }
    if let Some(v) = n.child_txt("Symlinks") {
        job.symlinks = if v.eq_ignore_ascii_case("follow") {
            Symlinks::Follow
        } else {
            Symlinks::Skip
        };
    }
    if let Some(v) = n.child_txt("IgnoreTimeShift") {
        job.time_shift_hours = parse_hours(v);
    }
}

fn dir_action(dir: Option<&str>, only: Option<bool>) -> CustomAction {
    // only: Some(true) = „tylko po lewej”, Some(false) = „tylko po prawej”, None = oba istnieją
    match (dir.map(str::to_lowercase).as_deref(), only) {
        (Some("right"), Some(true)) => CustomAction::CopyToRight,
        (Some("left"), Some(true)) => CustomAction::DeleteLeft,
        (Some("left"), Some(false)) => CustomAction::CopyToLeft,
        (Some("right"), Some(false)) => CustomAction::DeleteRight,
        (Some("right"), None) => CustomAction::CopyToRight,
        (Some("left"), None) => CustomAction::CopyToLeft,
        _ => CustomAction::Skip,
    }
}

fn apply_sync(job: &mut Job, n: &Node) {
    if let Some(v) = n.child_txt("Variant") {
        job.no_delete_left = false;
        job.no_delete_right = false;
        match v.to_lowercase().as_str() {
            "mirror" => job.mode = Mode::LeftToRight,
            "update" => {
                job.mode = Mode::LeftToRight;
                job.no_delete_right = true;
            }
            "custom" => {
                job.mode = Mode::Custom;
                let d = n.child("CustomDirections");
                let g = |k: &str| d.and_then(|d| d.child_txt(k));
                let content = job.compare == Compare::Content;
                let conflict = g("Conflict");
                let different = g("Different");
                // w trybie „zawartość” FreeFileSync zna tylko kategorię „Different” (bez nowszy/starszy)
                let diff_dir = if content {
                    different.or(conflict)
                } else {
                    conflict.or(different)
                };
                let mut rules = CustomRules {
                    left_only: dir_action(g("LeftOnly"), Some(true)),
                    right_only: dir_action(g("RightOnly"), Some(false)),
                    left_newer: dir_action(g("LeftNewer"), None),
                    right_newer: dir_action(g("RightNewer"), None),
                    different: dir_action(diff_dir, None),
                };
                if content {
                    rules.left_newer = rules.different;
                    rules.right_newer = rules.different;
                }
                job.custom = Some(rules);
            }
            _ => job.mode = Mode::TwoWay, // TwoWay / Automatic / nieznany: najbezpieczniejszy
        }
    }
    if let Some(v) = n.child_txt("DetectMovedFiles") {
        job.detect_moves = truthy(v);
    }
    if let Some(v) = n.child_txt("DeletionPolicy") {
        match v.to_lowercase().as_str() {
            "permanent" => job.trash_days = 0,
            _ => {
                job.trash_days = 30;
                if v.eq_ignore_ascii_case("versioning") {
                    if let Some(f) = n.child("VersioningFolder") {
                        let style = f.attr("Style").unwrap_or("").to_lowercase();
                        job.versioning = if style.starts_with("replace") {
                            Versioning::Replace
                        } else {
                            Versioning::Folders
                        };
                        let num = |k: &str| f.attr(k).and_then(|x| x.trim().parse::<u32>().ok());
                        if let Some(d) = num("MaxAge").filter(|d| (1..=36_500).contains(d)) {
                            job.trash_days = d;
                        }
                        if let Some(c) = num("MaxCount").filter(|c| *c > 0) {
                            job.max_versions = c;
                        }
                    }
                }
            }
        }
    }
}

/// Filtry (globalne albo lokalne dla pary): zwraca (include, exclude).
fn apply_filter(job: &mut Job, f: &Node, replace_include: bool) {
    let inc = collect_masks(f.child("Include"));
    if !(inc.is_empty() || inc == ["*"]) {
        if replace_include {
            job.include = inc;
        } else {
            job.include.extend(inc);
        }
    }
    for g in collect_masks(f.child("Exclude")) {
        if !job.exclude.contains(&g) {
            job.exclude.push(g);
        }
    }
    let size = |tag: &str| -> Option<u64> {
        let n = f.child(tag)?;
        let mult = unit_mult(n.attr("Unit"))?;
        let v: u64 = n.txt().parse().ok()?;
        v.checked_mul(mult)
    };
    if let Some(v) = size("SizeMin").filter(|v| *v > 0) {
        job.min_size = Some(v);
    }
    if let Some(v) = size("SizeMax").filter(|v| *v > 0) {
        job.max_size = Some(v);
    }
    if let Some(t) = f.child("TimeSpan") {
        match t.attr("Type").map(str::to_lowercase).as_deref() {
            Some("lastdays") => {
                if let Ok(d) = t.txt().parse::<u32>() {
                    if (1..=36_500).contains(&d) {
                        job.max_age_days = Some(d);
                    }
                }
            }
            Some("today") => job.max_age_days = Some(1),
            _ => {}
        }
    }
}

fn base_name(p: &str) -> String {
    p.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .to_string()
}

/// Importuje zadania z tekstu XML FreeFileSync (jedna para folderów = jedno zadanie).
pub fn parse_ffs_xml(xml: &str) -> Result<Vec<Job>, String> {
    parse_ffs_named(xml, None)
}

/// Jak `parse_ffs_xml`; `stem` (np. nazwa pliku bez rozszerzenia) służy do nazw zadań.
pub fn parse_ffs_named(xml: &str, stem: Option<&str>) -> Result<Vec<Job>, String> {
    if xml.len() as u64 > MAX_XML_BYTES {
        return Err("Plik FreeFileSync jest zbyt duży.".into());
    }
    let xml = xml.trim_start_matches('\u{feff}');
    let root = parse_xml(xml)?;
    if root.name != "FreeFileSync" {
        return Err("To nie jest plik FreeFileSync (.ffs_gui / .ffs_batch).".into());
    }
    let blank: Job = serde_json::from_value(serde_json::json!({
        "id": "ffs", "name": "ffs", "left": "-", "right": "-",
        "mode": "TwoWay", "conflict": "Newer", "schedule": {"type": "Manual"}
    }))
    .map_err(|e| e.to_string())?;
    let mut base = blank;
    base.enabled = false;
    base.run_before = None;
    base.run_after = None;
    // bez dokładnych danych FreeFileSync domyślnie nie wykrywa przeniesień
    base.detect_moves = false;
    if let Some(c) = root.child("Compare") {
        apply_compare(&mut base, c);
    }
    // tolerancja czasu bywa w ustawieniach globalnych albo bezpośrednio w pliku
    if let Some(t) = root.find("FileTimeTolerance") {
        if let Ok(v) = t.txt().parse::<i64>() {
            base.time_tolerance_secs = v.clamp(0, 86_400) as u32;
        }
    }
    if base.time_shift_hours == 0 {
        if let Some(t) = root.find("IgnoreTimeShift") {
            base.time_shift_hours = parse_hours(t.txt());
        }
    }
    if let Some(s) = root.child("Synchronize") {
        apply_sync(&mut base, s);
    }
    if let Some(f) = root.child("Filter") {
        apply_filter(&mut base, f, false);
    }

    let pairs: Vec<&Node> = root
        .child("FolderPairs")
        .map(|fp| fp.children_named("Pair").collect())
        .unwrap_or_default();
    let mut jobs: Vec<Job> = Vec::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for (idx, p) in pairs.iter().enumerate() {
        let left = p.child_txt("Left").unwrap_or("").to_string();
        let right = p.child_txt("Right").unwrap_or("").to_string();
        if left.is_empty() && right.is_empty() {
            continue; // pusty wiersz pary w FreeFileSync
        }
        if left.is_empty() || right.is_empty() {
            return Err(format!(
                "Para folderów nr {}: brakuje lewego albo prawego folderu.",
                idx + 1
            ));
        }
        let mut j = base.clone();
        j.left = left;
        j.right = right;
        if let Some(c) = p.child("Compare") {
            apply_compare(&mut j, c);
        }
        if let Some(s) = p.child("Synchronize") {
            apply_sync(&mut j, s);
        }
        if let Some(f) = p.child("LocalFilter") {
            apply_filter(&mut j, f, true);
        }
        let h = blake3::hash(format!("{now}|{idx}|{}|{}", j.left, j.right).as_bytes()).to_hex();
        j.id = format!("ffs-{}", &h[..12]);
        jobs.push(j);
    }
    if jobs.is_empty() {
        return Err("Plik FreeFileSync nie zawiera żadnej pary folderów.".into());
    }
    let n = jobs.len();
    for (i, j) in jobs.iter_mut().enumerate() {
        let auto = format!("{} ↔ {}", base_name(&j.left), base_name(&j.right));
        j.name = match (stem.map(str::trim).filter(|s| !s.is_empty()), n) {
            (Some(s), 1) => s.to_string(),
            (Some(s), _) => format!("{s} ({})", i + 1),
            (None, _) if auto.trim() != "↔" && !base_name(&j.left).is_empty() => auto,
            (None, _) => format!("Import z FreeFileSync {}", i + 1),
        };
        j.validate()
            .map_err(|e| format!("Para folderów nr {} („{}”): {e}", i + 1, j.name))?;
    }
    Ok(jobs)
}

/// Wczytuje plik `.ffs_gui` / `.ffs_batch` (UTF-8 z BOM lub bez, UTF-16 z BOM).
pub fn read_ffs_file(path: &Path) -> Result<Vec<Job>, String> {
    let md = std::fs::metadata(path)
        .map_err(|e| format!("Nie można odczytać pliku {}: {e}", path.display()))?;
    if md.len() > MAX_XML_BYTES {
        return Err("Plik FreeFileSync jest zbyt duży.".into());
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Nie można odczytać pliku {}: {e}", path.display()))?;
    let text = crate::rules::decode_text(&bytes);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty());
    parse_ffs_named(&text, stem.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(mask_to_globs("*.tmp"), ["*.tmp", "*.tmp/*", "*/*.tmp/*"]);
        assert_eq!(mask_to_globs("\\Foo\\"), ["Foo/*"]);
        assert_eq!(mask_to_globs("*\\bin\\"), ["*/bin/*", "*/*/bin/*"]);
        assert!(mask_to_globs("  ").is_empty());
    }

    #[test]
    fn entities_and_cdata() {
        let n = parse_xml("<a x=\"&lt;1&gt;\"><![CDATA[<b>]]>&amp;&#65;&#x42;</a>").unwrap();
        assert_eq!(n.attr("x"), Some("<1>"));
        assert_eq!(n.txt(), "<b>&AB");
    }

    #[test]
    fn broken_xml() {
        assert!(parse_xml("<a><b></a>").is_err());
        assert!(parse_xml("<a>").is_err());
        assert!(parse_xml("").is_err());
    }
}
