//! Silnik: skan obu stron, planowanie, wykonanie równoległe, zapis stanu (SQLite) i historii.
use crate::location::Opts;
use crate::location::{Entry, Location, OpError};
use crate::model::Job;
use crate::names;
use crate::plan::{mass_delete_guard, plan, same, Action, Meta, Tree};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Report {
    pub started: i64,
    pub dry_run: bool,
    pub scanned_left: usize,
    pub scanned_right: usize,
    pub copied: usize,
    pub deleted: usize,
    /// Pliki przeniesione/przemianowane bez ponownego kopiowania.
    pub moved: usize,
    pub conflicts: Vec<String>,
    /// Rozmiary i czasy obu wersji plików w konflikcie (podgląd do rozstrzygnięcia).
    pub conflict_details: Vec<ConflictDetail>,
    /// Pliki zajęte przez inne programy: zostaną ponowione.
    pub busy: Vec<String>,
    pub errors: Vec<String>,
    /// Plan działań (wypełniany w trybie symulacji i po wykonaniu).
    pub planned: Vec<(String, Action)>,
    /// Powód zatrzymania przez zabezpieczenia.
    pub blocked: Option<String>,
    /// Dlaczego VSS nie zadziałał (np. brak uprawnień administratora).
    pub vss_note: Option<String>,
}

impl Report {
    pub fn needs_retry(&self) -> bool {
        !self.busy.is_empty() || !self.errors.is_empty()
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ConflictDetail {
    pub path: String,
    pub left: Option<Meta>,
    pub right: Option<Meta>,
}

#[derive(Debug, Serialize)]
pub struct HistoryRow {
    pub ts: i64,
    pub job: String,
    pub kind: String,
    pub path: String,
    pub detail: String,
}

pub fn open_db(state_dir: &Path) -> anyhow::Result<Connection> {
    std::fs::create_dir_all(state_dir)?;
    let c = Connection::open(state_dir.join("state.db"))?;
    c.busy_timeout(std::time::Duration::from_secs(10))?;
    c.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS base(job TEXT, key TEXT, lsize INT, lmtime INT, rsize INT, rmtime INT, PRIMARY KEY(job,key));
         CREATE TABLE IF NOT EXISTS history(id INTEGER PRIMARY KEY, ts INT, job TEXT, kind TEXT, path TEXT, detail TEXT);
         CREATE INDEX IF NOT EXISTS hist_ts ON history(ts);",
    )?;
    Ok(c)
}

fn load_base(c: &Connection, job: &str) -> anyhow::Result<(Tree, Tree)> {
    let mut st = c.prepare("SELECT key,lsize,lmtime,rsize,rmtime FROM base WHERE job=?1")?;
    let rows = st.query_map([job], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })?;
    let (mut l, mut r) = (Tree::new(), Tree::new());
    for row in rows {
        let (k, ls, lm, rs, rm) = row?;
        l.insert(
            k.clone(),
            Meta {
                size: ls as u64,
                mtime: lm,
            },
        );
        r.insert(
            k,
            Meta {
                size: rs as u64,
                mtime: rm,
            },
        );
    }
    Ok((l, r))
}

pub fn history(
    state_dir: &Path,
    job: Option<&str>,
    limit: usize,
) -> anyhow::Result<Vec<HistoryRow>> {
    let c = open_db(state_dir)?;
    let mut st = c.prepare(
        "SELECT ts,job,kind,path,detail FROM history WHERE (?1 IS NULL OR job=?1) ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = st.query_map(params![job, limit as i64], |r| {
        Ok(HistoryRow {
            ts: r.get(0)?,
            job: r.get(1)?,
            kind: r.get(2)?,
            path: r.get(3)?,
            detail: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Kasuje stan zapamiętany dla zadania (po zmianie lokalizacji).
pub fn reset_state(state_dir: &Path, job: &str) -> anyhow::Result<()> {
    let c = open_db(state_dir)?;
    c.execute("DELETE FROM base WHERE job=?1", [job])?;
    Ok(())
}

enum Out {
    Moved {
        old: String,
        key: String,
        l: Meta,
        r: Meta,
    },
    Synced {
        key: String,
        l: Meta,
        r: Meta,
        kind: &'static str,
    },
    Removed {
        key: String,
    },
    Conflict(String, Meta, Meta),
    Busy(String, String),
    Err(String, String),
}

struct Ctx<'a> {
    l: &'a Location,
    r: &'a Location,
    ls: &'a BTreeMap<String, Entry>,
    rs: &'a BTreeMap<String, Entry>,
    trash: &'a str, // pusty = bez kosza
    date: &'a str,
    o: &'a Opts,
}

/// Para „usunięty plik + nowy plik o tym samym rozmiarze i czasie” to przeniesienie: po stronie docelowej
/// wystarczy zmienić ścieżkę zamiast kopiować od nowa (oszczędza transfer, zwłaszcza w chmurze).
fn detect_moves(
    actions: &mut BTreeMap<String, Action>,
    lt: &Tree,
    rt: &Tree,
    bl: &Tree,
    br: &Tree,
) {
    fn pair(
        actions: &mut BTreeMap<String, Action>,
        known: &Tree,
        del: Action,
        copy: Action,
        old_tree: &Tree,
        new_tree: &Tree,
        to_right: bool,
    ) {
        let like = |a: &Meta, b: &Meta| a.size == b.size && (a.mtime - b.mtime).abs() <= 2;
        let olds: Vec<(String, Meta)> = actions
            .iter()
            .filter(|(k, a)| **a == del && known.contains_key(*k)) // tylko pliki, które wcześniej były zsynchronizowane
            .filter_map(|(k, _)| old_tree.get(k).map(|m| (k.clone(), *m)))
            .collect();
        let news: Vec<(String, Meta)> = actions
            .iter()
            .filter(|(k, a)| **a == copy && !old_tree.contains_key(*k))
            .filter_map(|(k, _)| new_tree.get(k).map(|m| (k.clone(), *m)))
            .collect();
        for (nk, nm) in &news {
            if nm.size == 0 {
                continue;
            }
            let cands: Vec<&(String, Meta)> = olds.iter().filter(|(_, om)| like(om, nm)).collect();
            // tylko jednoznaczne dopasowanie w obie strony
            if cands.len() != 1 || news.iter().filter(|(_, m2)| like(m2, &cands[0].1)).count() != 1
            {
                continue;
            }
            let from = cands[0].0.clone();
            actions.remove(&from);
            let a = if to_right {
                Action::RenameRight { from }
            } else {
                Action::RenameLeft { from }
            };
            actions.insert(nk.clone(), a);
        }
    }
    pair(
        actions,
        br,
        Action::DeleteRight,
        Action::CopyToRight,
        rt,
        lt,
        true,
    );
    pair(
        actions,
        bl,
        Action::DeleteLeft,
        Action::CopyToLeft,
        lt,
        rt,
        false,
    );
}

/// Przenosi plik po stronie docelowej; gdy się nie da, kopiuje nowy i usuwa stary.
fn move_one(
    src: &Location,
    src_new: &Entry,
    dst: &Location,
    dst_old: &Entry,
    trash: &str,
    o: &Opts,
) -> Result<(Meta, Meta), OpError> {
    match dst.rename(&dst_old.real, &src_new.real) {
        Ok(()) => Ok((
            src_new.meta,
            dst.stat(&src_new.real).unwrap_or(dst_old.meta),
        )),
        Err(_) => {
            let r = copy_one(src, src_new, dst, None, trash, o)?;
            dst.remove(&dst_old.real, trash)?;
            Ok(r)
        }
    }
}

fn err_out(key: &str, e: OpError) -> Out {
    match e {
        OpError::Busy(m) => Out::Busy(key.into(), m),
        OpError::Failed(m) => Out::Err(key.into(), m),
    }
}

/// Kopiuje `src` -> `dst`; zwraca (meta źródła, meta celu po kopii).
fn copy_one(
    src: &Location,
    se: &Entry,
    dst: &Location,
    de: Option<&Entry>,
    trash: &str,
    o: &Opts,
) -> Result<(Meta, Meta), OpError> {
    let dst_real = de
        .map(|d| d.real.clone())
        .unwrap_or_else(|| se.real.clone());
    if de.is_some() && !trash.is_empty() {
        // kopia zapasowa starej wersji do kosza (nie przenosimy: nieudane kopiowanie nie może zostawić celu bez pliku)
        dst.backup(&dst_real, trash)?;
    }
    dst.copy_from(src, &se.real, &dst_real, o)?;
    let dm = dst.stat(&dst_real).unwrap_or(se.meta);
    Ok((se.meta, dm))
}

fn exec(key: &str, a: &Action, c: &Ctx) -> Out {
    let (le, re) = (c.ls.get(key), c.rs.get(key));
    match a {
        Action::CopyToRight => match (le, copy_one(c.l, le.unwrap(), c.r, re, c.trash, c.o)) {
            (_, Ok((l, r))) => Out::Synced {
                key: key.into(),
                l,
                r,
                kind: "kopiowanie →",
            },
            (_, Err(e)) => err_out(key, e),
        },
        Action::CopyToLeft => match copy_one(c.r, re.unwrap(), c.l, le, c.trash, c.o) {
            Ok((r, l)) => Out::Synced {
                key: key.into(),
                l,
                r,
                kind: "kopiowanie ←",
            },
            Err(e) => err_out(key, e),
        },
        Action::RenameRight { from } => match (c.rs.get(from), le) {
            (Some(old), Some(new)) => match move_one(c.l, new, c.r, old, c.trash, c.o) {
                Ok((l, r)) => Out::Moved {
                    old: from.clone(),
                    key: key.into(),
                    l,
                    r,
                },
                Err(e) => err_out(key, e),
            },
            _ => Out::Err(key.into(), "przeniesienie: brak pliku źródłowego".into()),
        },
        Action::RenameLeft { from } => match (c.ls.get(from), re) {
            (Some(old), Some(new)) => match move_one(c.r, new, c.l, old, c.trash, c.o) {
                Ok((r, l)) => Out::Moved {
                    old: from.clone(),
                    key: key.into(),
                    l,
                    r,
                },
                Err(e) => err_out(key, e),
            },
            _ => Out::Err(key.into(), "przeniesienie: brak pliku źródłowego".into()),
        },
        Action::DeleteLeft => match c.l.remove(&le.unwrap().real, c.trash) {
            Ok(()) => Out::Removed { key: key.into() },
            Err(e) => err_out(key, e),
        },
        Action::DeleteRight => match c.r.remove(&re.unwrap().real, c.trash) {
            Ok(()) => Out::Removed { key: key.into() },
            Err(e) => err_out(key, e),
        },
        Action::Conflict { keep_both: false } => {
            Out::Conflict(le.unwrap().real.clone(), le.unwrap().meta, re.unwrap().meta)
        }
        Action::Conflict { keep_both: true } => {
            let (le, re) = (le.unwrap(), re.unwrap());
            // wersja z prawej trafia na lewą pod nazwą konfliktową, wersja z lewej wygrywa pod oryginalną nazwą
            let alias = Entry {
                real: names::conflict_name(&re.real, c.date),
                meta: re.meta,
            };
            let src_alias = Entry {
                real: re.real.clone(),
                meta: re.meta,
            };
            if let Err(e) = c.l.copy_from(c.r, &src_alias.real, &alias.real, c.o) {
                return err_out(key, e);
            }
            match copy_one(c.l, le, c.r, Some(re), c.trash, c.o) {
                Ok((l, r)) => Out::Synced {
                    key: key.into(),
                    l,
                    r,
                    kind: "konflikt: obie wersje",
                },
                Err(e) => err_out(key, e),
            }
        }
    }
}

/// Postęp wykonywania: (zrobione, wszystkie, bieżący plik).
pub type ProgressFn<'a> = &'a (dyn Fn(usize, usize, &str) + Sync);

/// Reguły zadania + maski z plików `.syncignore` z korzeni obu lokalizacji (wędrują razem z danymi).
fn rules_with_ignore(job: &Job, l: &Location, r: &Location) -> crate::rules::RuleSet {
    let mut rules = job.rules();
    for loc in [l, r] {
        if let Some(text) = loc.read_text(".syncignore") {
            rules.exclude.extend(crate::rules::parse_ignore(&text));
        }
    }
    rules
}

pub fn run_job(job: &Job, state_dir: &Path, dry: bool, force: bool) -> anyhow::Result<Report> {
    run_job_with(job, state_dir, dry, force, None)
}

pub fn run_job_with(
    job: &Job,
    state_dir: &Path,
    dry: bool,
    force: bool,
    progress: Option<ProgressFn>,
) -> anyhow::Result<Report> {
    let mut rep = Report {
        started: chrono::Local::now().timestamp(),
        dry_run: dry,
        ..Default::default()
    };
    let (l, r) = (Location::parse(&job.left), Location::parse(&job.right));
    let rules = rules_with_ignore(job, &l, &r);

    let (lres, rres) = std::thread::scope(|s| {
        let a = s.spawn(|| l.list(&rules));
        let b = s.spawn(|| r.list(&rules));
        (a.join().unwrap(), b.join().unwrap())
    });
    let (ll, rl) = match (lres, rres) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) => anyhow::bail!("lewa strona: {e}"),
        (_, Err(e)) => anyhow::bail!("prawa strona: {e}"),
    };
    rep.scanned_left = ll.files.len();
    rep.scanned_right = rl.files.len();
    for c in ll.collisions.iter().chain(rl.collisions.iter()) {
        rep.errors
            .push(format!("kolizja nazw (wielkość liter/Unicode): {c}"));
    }

    let db = open_db(state_dir)?;
    let (bl, br) = load_base(&db, &job.id)?;
    let tree =
        |m: &BTreeMap<String, Entry>| m.iter().map(|(k, e)| (k.clone(), e.meta)).collect::<Tree>();
    let (lt, rt) = (tree(&ll.files), tree(&rl.files));
    let mut actions = plan(&lt, &rt, &bl, &br, job.options());
    detect_moves(&mut actions, &lt, &rt, &bl, &br);
    rep.planned = actions
        .iter()
        .map(|(k, a)| (k.clone(), a.clone()))
        .collect();

    if !force {
        if (lt.is_empty() && !bl.is_empty()) || (rt.is_empty() && !br.is_empty()) {
            rep.blocked = Some(
                "jedna ze stron jest pusta, choć wcześniej były tam pliki (odpięty dysk?)".into(),
            );
        } else if mass_delete_guard(&actions, lt.len().max(rt.len()), job.guard_pct as usize) {
            rep.blocked = Some(format!("plan usuwa ponad {}% plików", job.guard_pct));
        }
    }
    if dry || rep.blocked.is_some() {
        return Ok(rep);
    }

    let now = chrono::Local::now();
    let trash = if job.trash_days > 0 {
        now.format("%Y-%m-%d_%H%M%S").to_string()
    } else {
        String::new()
    };
    let date = now.format("%Y-%m-%d").to_string();
    let opts = Opts::new(job.use_vss, job.verify, &job.bandwidth, job.threads);
    let ctx = Ctx {
        l: &l,
        r: &r,
        ls: &ll.files,
        rs: &rl.files,
        trash: &trash,
        date: &date,
        o: &opts,
    };

    let total = actions.len();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let queue = Mutex::new(
        actions
            .iter()
            .map(|(k, a)| (k.clone(), a.clone()))
            .collect::<Vec<_>>(),
    );
    let results = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..job.threads.clamp(1, 16) {
            s.spawn(|| loop {
                let item = queue.lock().unwrap().pop();
                let Some((k, a)) = item else { break };
                let o = exec(&k, &a, &ctx);
                results.lock().unwrap().push(o);
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if let Some(p) = progress {
                    p(n, total, &k);
                }
            });
        }
    });

    let ts = chrono::Local::now().timestamp();
    let tx = db.unchecked_transaction()?;
    let hist = |kind: &str, path: &str, detail: &str| {
        tx.execute(
            "INSERT INTO history(ts,job,kind,path,detail) VALUES(?1,?2,?3,?4,?5)",
            params![ts, job.id, kind, path, detail],
        )
    };
    for o in results.into_inner().unwrap() {
        match o {
            Out::Synced { key, l, r, kind } => {
                rep.copied += 1;
                tx.execute(
                    "INSERT OR REPLACE INTO base(job,key,lsize,lmtime,rsize,rmtime) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![job.id, key, l.size as i64, l.mtime, r.size as i64, r.mtime],
                )?;
                hist(kind, &key, "")?;
            }
            Out::Moved { old, key, l, r } => {
                rep.moved += 1;
                tx.execute(
                    "DELETE FROM base WHERE job=?1 AND key=?2",
                    params![job.id, old],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO base(job,key,lsize,lmtime,rsize,rmtime) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![job.id, key, l.size as i64, l.mtime, r.size as i64, r.mtime],
                )?;
                hist("przeniesienie", &key, &format!("z: {old}"))?;
            }
            Out::Removed { key } => {
                rep.deleted += 1;
                tx.execute(
                    "DELETE FROM base WHERE job=?1 AND key=?2",
                    params![job.id, key],
                )?;
                hist("usunięcie", &key, "")?;
            }
            Out::Conflict(p, l, r) => {
                hist("konflikt", &p, "wymaga decyzji")?;
                rep.conflict_details.push(ConflictDetail {
                    path: p.clone(),
                    left: Some(l),
                    right: Some(r),
                });
                rep.conflicts.push(p);
            }
            Out::Busy(p, m) => {
                hist("zajęty", &p, &m)?;
                rep.busy.push(p);
            }
            Out::Err(p, m) => {
                hist("błąd", &p, &m)?;
                rep.errors.push(format!("{p}: {m}"));
            }
        }
    }
    // pliki zgodne po obu stronach bez działań: odśwież migawkę; brak po obu stronach: usuń wpis
    let keys: std::collections::BTreeSet<&String> =
        lt.keys().chain(rt.keys()).chain(bl.keys()).collect();
    for k in keys {
        if actions.contains_key(k) {
            continue;
        }
        match (lt.get(k), rt.get(k)) {
            (Some(a), Some(b)) if same(a, b) => {
                tx.execute(
                    "INSERT OR REPLACE INTO base(job,key,lsize,lmtime,rsize,rmtime) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![job.id, k, a.size as i64, a.mtime, b.size as i64, b.mtime],
                )?;
            }
            (None, None) => {
                tx.execute(
                    "DELETE FROM base WHERE job=?1 AND key=?2",
                    params![job.id, k],
                )?;
            }
            _ => {}
        }
    }
    tx.execute("DELETE FROM history WHERE ts < ?1", [ts - 90 * 86400])?;
    tx.commit()?;

    rep.vss_note = opts.vss.note();
    if job.trash_days > 0 {
        l.purge_trash(job.trash_days, job.max_versions);
        r.purge_trash(job.trash_days, job.max_versions);
    }
    Ok(rep)
}

/// Ręczne rozstrzygnięcie konfliktu: wygrywa wybrana strona (przegrana wersja trafia do kosza).
pub fn resolve_conflict(
    job: &Job,
    state_dir: &Path,
    path: &str,
    left_wins: bool,
) -> anyhow::Result<()> {
    let (l, r) = (Location::parse(&job.left), Location::parse(&job.right));
    let rules = rules_with_ignore(job, &l, &r);
    let key = names::key(path);
    let (ll, rl) = (
        l.list(&rules).map_err(anyhow::Error::msg)?,
        r.list(&rules).map_err(anyhow::Error::msg)?,
    );
    let (le, re) = match (ll.files.get(&key), rl.files.get(&key)) {
        (Some(a), Some(b)) => (a, b),
        _ => anyhow::bail!("plik nie istnieje już po obu stronach"),
    };
    let opts = Opts::new(job.use_vss, job.verify, &job.bandwidth, job.threads);
    let trash = if job.trash_days > 0 {
        chrono::Local::now().format("%Y-%m-%d_%H%M%S").to_string()
    } else {
        String::new()
    };
    let (lm, rm) = if left_wins {
        copy_one(&l, le, &r, Some(re), &trash, &opts).map_err(|e| anyhow::anyhow!(e.to_string()))?
    } else {
        let (b, a) = copy_one(&r, re, &l, Some(le), &trash, &opts)
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        (a, b)
    };
    let db = open_db(state_dir)?;
    db.execute(
        "INSERT OR REPLACE INTO base(job,key,lsize,lmtime,rsize,rmtime) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            job.id,
            key,
            lm.size as i64,
            lm.mtime,
            rm.size as i64,
            rm.mtime
        ],
    )?;
    db.execute(
        "INSERT INTO history(ts,job,kind,path,detail) VALUES(?1,?2,'konflikt rozstrzygnięty',?3,?4)",
        params![chrono::Local::now().timestamp(), job.id, path, if left_wins { "wygrała lewa strona" } else { "wygrała prawa strona" }],
    )?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct TrashItem {
    /// "left" | "right"
    pub side: String,
    pub stamp: String,
    pub path: String,
    pub size: u64,
}

fn side_loc(job: &Job, side: &str) -> anyhow::Result<Location> {
    match side {
        "left" => Ok(Location::parse(&job.left)),
        "right" => Ok(Location::parse(&job.right)),
        _ => anyhow::bail!("Nieznana strona zadania: {side} (dozwolone: left, right)"),
    }
}

/// Kosz i wersje plików obu stron zadania (strona niedostępna jest pomijana, o ile druga działa).
pub fn trash_list(job: &Job) -> anyhow::Result<Vec<TrashItem>> {
    let (mut out, mut errs) = (Vec::new(), Vec::new());
    for side in ["left", "right"] {
        match side_loc(job, side)?.list_trash() {
            Ok(v) => out.extend(v.into_iter().map(|(stamp, path, size)| TrashItem {
                side: side.into(),
                stamp,
                path,
                size,
            })),
            Err(e) => errs.push(format!("{side}: {e}")),
        }
    }
    if out.is_empty() && errs.len() == 2 {
        anyhow::bail!(errs.join("; "));
    }
    Ok(out)
}

pub fn trash_restore(job: &Job, side: &str, stamp: &str, path: &str) -> anyhow::Result<String> {
    side_loc(job, side)?
        .restore_trash(stamp, path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

pub fn trash_delete(
    job: &Job,
    side: &str,
    stamp: Option<&str>,
    path: Option<&str>,
) -> anyhow::Result<()> {
    side_loc(job, side)?
        .delete_trash(stamp, path)
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}

/// Podfoldery wskazanego folderu jednej ze stron (wybór podfolderów w kreatorze).
pub fn list_dirs(job_left: &str, rel: &str) -> anyhow::Result<Vec<String>> {
    Location::parse(job_left)
        .list_dirs(rel)
        .map_err(anyhow::Error::msg)
}
