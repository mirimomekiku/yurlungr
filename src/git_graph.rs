use std::{collections::HashMap, path::Path};

use anyhow::Result;
use git2::{Delta, Oid, Patch, Repository, Sort, Time};

pub struct GraphRow {
    pub graph: String,
    /// Abbreviated id for the list.
    pub id: String,
    pub full_id: String,
    pub summary: String,
    pub author: String,
    /// Author date, `YYYY-MM-DD`.
    pub date: String,
    pub refs: Vec<String>,
}

/// One file touched by a commit.
pub struct FileChange {
    /// `A`dded, `M`odified, `D`eleted, `R`enamed, `C`opied, `T`ype changed.
    pub status: char,
    /// For renames, `old -> new`.
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

pub struct CommitDetail {
    pub id: String,
    pub author: String,
    pub author_time: String,
    pub committer: String,
    pub commit_time: String,
    pub parents: Vec<String>,
    pub message: String,
    pub files: Vec<FileChange>,
    /// All changed files, including ones beyond `files`.
    pub files_total: usize,
    /// Line totals; `None` when the commit is too big to count cheaply.
    pub lines: Option<(usize, usize)>,
}

/// Past this many files per-file line counts are skipped to keep the UI responsive.
const MAX_STAT_FILES: usize = 300;
/// Past this many files the list itself is cut.
const MAX_LISTED_FILES: usize = 2000;

/// `YYYY-MM-DD`, or `YYYY-MM-DD HH:MM +ZZZZ` with the clock, in the commit's own timezone.
pub fn format_time(secs: i64, offset_minutes: i32, with_clock: bool) -> String {
    let local = secs + i64::from(offset_minutes) * 60;
    let (days, rem) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    let date = format!("{y:04}-{m:02}-{d:02}");
    if !with_clock {
        return date;
    }
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let off = offset_minutes.abs();
    format!("{date} {:02}:{:02} {sign}{:02}{:02}", rem / 3600, rem % 3600 / 60, off / 60, off % 60)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn time_of(t: Time, with_clock: bool) -> String {
    format_time(t.seconds(), t.offset_minutes(), with_clock)
}

fn person(sig: &git2::Signature) -> String {
    format!("{} <{}>", sig.name().unwrap_or("?"), sig.email().unwrap_or("?"))
}

/// Who wrote and committed a commit, when, and which files it changed (against
/// its first parent, or the empty tree for a root commit).
pub fn detail(path: &Path, full_id: &str) -> Result<CommitDetail> {
    let repo = Repository::discover(path)?;
    let commit = repo.find_commit(Oid::from_str(full_id)?)?;
    let tree = commit.tree()?;
    let parent_tree = commit.parent(0).ok().and_then(|p| p.tree().ok());
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    // Rename detection is skipped on huge commits (it is quadratic).
    let counting = diff.deltas().len() <= MAX_STAT_FILES;
    if counting {
        diff.find_similar(None)?;
    }
    let files_total = diff.deltas().len();

    let mut files = Vec::new();
    let mut totals = (0usize, 0usize);
    for i in 0..diff.deltas().len().min(MAX_LISTED_FILES) {
        let Some(delta) = diff.get_delta(i) else { continue };
        let status = match delta.status() {
            Delta::Added => 'A',
            Delta::Deleted => 'D',
            Delta::Modified => 'M',
            Delta::Renamed => 'R',
            Delta::Copied => 'C',
            Delta::Typechange => 'T',
            _ => '?',
        };
        let name =
            |f: git2::DiffFile| f.path().map(|p| p.display().to_string()).unwrap_or_default();
        let path = match delta.status() {
            Delta::Renamed | Delta::Copied => {
                format!("{} -> {}", name(delta.old_file()), name(delta.new_file()))
            }
            Delta::Deleted => name(delta.old_file()),
            _ => name(delta.new_file()),
        };
        let (mut added, mut deleted, mut binary) = (0, 0, false);
        if counting {
            match Patch::from_diff(&diff, i)? {
                Some(patch) => {
                    let (_, a, d) = patch.line_stats()?;
                    (added, deleted) = (a, d);
                }
                None => binary = true,
            }
            totals = (totals.0 + added, totals.1 + deleted);
        }
        files.push(FileChange { status, path, added, deleted, binary });
    }

    let author = commit.author();
    let committer = commit.committer();
    Ok(CommitDetail {
        id: commit.id().to_string(),
        author: person(&author),
        author_time: time_of(author.when(), true),
        committer: person(&committer),
        commit_time: time_of(committer.when(), true),
        parents: commit.parent_ids().map(|p| p.to_string()[..7].to_string()).collect(),
        message: commit.message().unwrap_or("").trim_end().to_string(),
        files,
        files_total,
        lines: counting.then_some(totals),
    })
}

/// Load up to `limit` commits across all local branches, with an ASCII lane graph.
pub fn load(path: &Path, limit: usize) -> Result<Vec<GraphRow>> {
    let repo = Repository::discover(path)?;

    let mut walk = repo.revwalk()?;
    walk.set_sorting(Sort::TOPOLOGICAL | Sort::TIME)?;
    walk.push_glob("refs/heads/*")?;
    let _ = walk.push_head();

    let mut decorations: HashMap<Oid, Vec<String>> = HashMap::new();
    for r in repo.references()?.flatten() {
        if let (Ok(name), Ok(commit)) = (r.shorthand(), r.peel_to_commit()) {
            decorations.entry(commit.id()).or_default().push(name.to_string());
        }
    }

    let mut lanes: Vec<Option<Oid>> = Vec::new();
    let mut rows = Vec::new();

    for oid in walk.take(limit) {
        let commit = repo.find_commit(oid?)?;
        let id = commit.id();

        let col = match lanes.iter().position(|l| *l == Some(id)) {
            Some(c) => c,
            None => place(&mut lanes, id),
        };

        let graph = (0..lanes.len())
            .map(|i| match (i == col, lanes[i]) {
                (true, _) => "● ",
                (false, Some(_)) => "│ ",
                (false, None) => "  ",
            })
            .collect::<String>();

        // Other lanes waiting on this commit converge here.
        for (i, lane) in lanes.iter_mut().enumerate() {
            if i != col && *lane == Some(id) {
                *lane = None;
            }
        }
        let mut parents = commit.parent_ids();
        lanes[col] = parents.next();
        for p in parents {
            if !lanes.contains(&Some(p)) {
                place(&mut lanes, p);
            }
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }

        rows.push(GraphRow {
            graph,
            id: id.to_string()[..7].to_string(),
            full_id: id.to_string(),
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            date: time_of(commit.author().when(), false),
            refs: decorations.get(&id).cloned().unwrap_or_default(),
        });
    }
    Ok(rows)
}

fn place(lanes: &mut Vec<Option<Oid>>, id: Oid) -> usize {
    match lanes.iter().position(Option::is_none) {
        Some(i) => {
            lanes[i] = Some(id);
            i
        }
        None => {
            lanes.push(Some(id));
            lanes.len() - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use git2::{IndexAddOption, Repository, Signature, Time};

    use super::*;

    #[test]
    fn formats_dates_in_the_commits_own_timezone() {
        assert_eq!(format_time(0, 0, false), "1970-01-01");
        assert_eq!(format_time(1_700_000_000, 0, true), "2023-11-14 22:13 +0000");
        assert_eq!(format_time(1_700_000_000, 480, true), "2023-11-15 06:13 +0800");
        assert_eq!(format_time(1_700_000_000, -330, true), "2023-11-14 16:43 -0530");
        assert_eq!(format_time(951_782_400, 0, false), "2000-02-29", "leap day");
        assert_eq!(format_time(-86_400, 0, false), "1969-12-31");
    }

    fn commit(repo: &Repository, msg: &str, author: (&str, &str), committer: (&str, &str)) -> Oid {
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.update_all(["*"], None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let when = Time::new(1_700_000_000, 60);
        let a = Signature::new(author.0, author.1, &when).unwrap();
        let c = Signature::new(committer.0, committer.1, &when).unwrap();
        let parents: Vec<_> =
            repo.head().ok().and_then(|h| h.peel_to_commit().ok()).into_iter().collect();
        let refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &a, &c, msg, &tree, &refs).unwrap()
    }

    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).unwrap();
    }

    #[test]
    fn detail_reports_people_and_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        write(dir.path(), "keep.txt", "one\ntwo\n");
        write(dir.path(), "gone.txt", "bye\n");
        write(
            dir.path(),
            "old-name.txt",
            "a fairly long file\nwith several lines\nthat survive a rename\n",
        );
        let first = commit(&repo, "first", ("Ann", "ann@x.io"), ("Ann", "ann@x.io"));

        write(dir.path(), "keep.txt", "one\nTWO\nthree\n");
        write(dir.path(), "new.txt", "hello\n");
        fs::remove_file(dir.path().join("gone.txt")).unwrap();
        fs::rename(dir.path().join("old-name.txt"), dir.path().join("new-name.txt")).unwrap();
        let second = commit(&repo, "second\n\nlong body", ("Ann", "ann@x.io"), ("Bob", "bob@x.io"));

        let d = detail(dir.path(), &second.to_string()).unwrap();
        assert_eq!(d.id, second.to_string());
        assert_eq!(d.author, "Ann <ann@x.io>");
        assert_eq!(d.committer, "Bob <bob@x.io>", "committer is reported separately");
        assert_eq!(d.author_time, "2023-11-14 23:13 +0100");
        assert_eq!(d.parents, [first.to_string()[..7].to_string()]);
        assert_eq!(d.message, "second\n\nlong body");
        assert_eq!(d.files_total, 4);

        let by = |p: &str| d.files.iter().find(|f| f.path == p).unwrap_or_else(|| panic!("{p}"));
        let keep = by("keep.txt");
        assert_eq!((keep.status, keep.added, keep.deleted), ('M', 2, 1));
        assert_eq!((by("new.txt").status, by("new.txt").added), ('A', 1));
        assert_eq!((by("gone.txt").status, by("gone.txt").deleted), ('D', 1));
        assert_eq!(by("old-name.txt -> new-name.txt").status, 'R');
        assert_eq!(d.lines, Some((3, 2)), "+2 +1 added; -1 -1 deleted");
    }

    #[test]
    fn root_commit_lists_everything_as_added() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        write(dir.path(), "a.txt", "x\n");
        write(dir.path(), "b.txt", "y\n");
        let id = commit(&repo, "root", ("Ann", "a@x"), ("Ann", "a@x"));
        let d = detail(dir.path(), &id.to_string()).unwrap();
        assert!(d.parents.is_empty());
        assert_eq!(d.files.len(), 2);
        assert!(d.files.iter().all(|f| f.status == 'A'));
    }

    #[test]
    fn load_rows_carry_full_id_and_date() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        write(dir.path(), "a.txt", "x\n");
        let id = commit(&repo, "root", ("Ann", "a@x"), ("Ann", "a@x"));
        let rows = load(dir.path(), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].full_id, id.to_string());
        assert_eq!(rows[0].id, id.to_string()[..7]);
        assert_eq!(rows[0].date, "2023-11-14");
    }
}
