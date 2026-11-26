use clap::Args as ClapArgs;

use crate::annotation::Record;
use crate::cli::targets;
use crate::compact as compact_lib;
use crate::qual_file;

#[derive(ClapArgs)]
pub struct Args {
    /// The artifact to compact, relative to the current directory
    /// (required unless --all). Only its records are compacted; other
    /// subjects sharing its .qual file are left unchanged.
    pub artifact: Option<String>,

    /// Compact all .qual files in the repo
    #[arg(long)]
    pub all: bool,

    /// Collapse to a single epoch annotation
    #[arg(long)]
    pub snapshot: bool,

    /// Preview without writing
    #[arg(long)]
    pub dry_run: bool,

    /// Disable .gitignore and .qualignore filtering
    #[arg(long)]
    pub no_ignore: bool,
}

pub fn run(args: Args) -> crate::Result<()> {
    if args.all {
        return run_all(&args);
    }

    let artifact = args
        .artifact
        .as_deref()
        .ok_or_else(|| crate::Error::Validation("artifact is required (or use --all)".into()))?;

    let locator = targets::Locator::from_cwd()?;
    let subject = locator.subject(artifact)?;
    let qual_path = locator.existing_qual_file(&subject).ok_or_else(|| {
        crate::Error::Validation(format!(
            "No .qual file found containing annotations for '{subject}'"
        ))
    })?;

    let qf = qual_file::parse(&qual_path)?;
    compact_one(&qf, Some(&subject), args.snapshot, args.dry_run)?;

    Ok(())
}

fn run_all(args: &Args) -> crate::Result<()> {
    let qual_files = targets::discover_project(!args.no_ignore)?;

    if qual_files.is_empty() {
        println!("No .qual files found.");
        return Ok(());
    }

    for qf in &qual_files {
        compact_one(qf, None, args.snapshot, args.dry_run)?;
    }

    Ok(())
}

/// Compact `qf`. With `subject`, only that subject's records are compacted;
/// records of other subjects sharing the file (a directory-level `.qual`)
/// are kept unchanged and in place, and the compacted records take the
/// position of the subject's first record.
fn compact_one(
    qf: &qual_file::QualFile,
    subject: Option<&str>,
    snapshot: bool,
    dry_run: bool,
) -> crate::Result<()> {
    let in_scope = |r: &Record| subject.is_none_or(|s| r.subject() == s);
    let scoped = qual_file::QualFile {
        path: qf.path.clone(),
        subject: qf.subject.clone(),
        records: qf.records.iter().filter(|r| in_scope(r)).cloned().collect(),
    };
    let (compacted, result) = if snapshot {
        compact_lib::snapshot(&scoped)
    } else {
        compact_lib::prune(&scoped)
    };

    if result.pruned == 0 {
        println!(
            "  {}: {} records, nothing to compact",
            qf.path.display(),
            result.before
        );
        return Ok(());
    }

    if snapshot {
        println!(
            "  {}: {} -> {} record (epoch)",
            qf.path.display(),
            result.before,
            result.after,
        );
    } else {
        println!(
            "  {}: {} -> {} records ({} superseded, pruned)",
            qf.path.display(),
            result.before,
            result.after,
            result.pruned,
        );
    }

    if !dry_run {
        let mut compacted = Some(compacted.records);
        let mut records = Vec::with_capacity(qf.records.len());
        for r in &qf.records {
            if !in_scope(r) {
                records.push(r.clone());
            } else if let Some(c) = compacted.take() {
                records.extend(c);
            }
        }
        qual_file::write_all(&qf.path, &records)?;
    } else {
        println!("  (dry run — no changes written)");
    }

    Ok(())
}
