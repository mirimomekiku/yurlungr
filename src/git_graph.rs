use std::{collections::HashMap, path::Path};

use anyhow::Result;
use git2::{Oid, Repository, Sort};

pub struct GraphRow {
    pub graph: String,
    pub id: String,
    pub summary: String,
    pub author: String,
    pub refs: Vec<String>,
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
            summary: commit.summary().ok().flatten().unwrap_or("").to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
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
