use std::env::var;
use std::fs::{ read_dir, remove_dir_all };
use std::hash::{ Hash, Hasher };
use std::io::{ stdin, Write };
use std::path::{ Path, PathBuf };
use std::process::{ Command, Stdio };

use clap::Parser;
use flate2::{ Compression, write::GzEncoder, read::GzDecoder };
use gpgme::{ Context, Protocol };
use openssh::{ Session, SessionBuilder, KnownHosts };
use tar::{ Builder, Archive };
use tokio::io::{ AsyncReadExt, AsyncWriteExt };
use toml::Value;
use twox_hash::XxHash64;
use walkdir::WalkDir;

pub mod config;

use config::{ Config, load_configuration };

/// Test whether the local sync directory exists.
async fn confirm_local_exists(home: &Path, dir: &Path)
-> std::io::Result<bool> {
    for f in read_dir(home)? {
        let path = match f {
            Ok(x)  => x,
            Err(_) => continue,
        };
        let path = path.path();
        if path == dir && path.is_dir() {
            return Ok(true)
        }
    }
    Ok(false)
}

/// Call Unison on the local and remote folder.
async fn unison(local: &Path, remote: &Path, batch: bool)
-> Result<bool, std::io::Error> {
    let mut cmd = Command::new("unison");
    if batch {
        cmd
            .arg(local)
            .arg(remote)
            .arg("-batch");
    } else {
        cmd
            .arg(local)
            .arg(remote);
    };
    let cmd = cmd.status()?;
    Ok(cmd.success())
}

/// Get the contents of a remote file.
async fn read_remote_file(s: &mut Session, file: &str)
-> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut sftp = s.sftp();
    let mut f = sftp.read_from(file).await?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).await?;
    f.close().await?;
    Ok(b)
}

/// Decrypt the remote archive's data.
async fn decrypt(bytes: &[u8], gpgbin: &Option<Value>)
-> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut ctx = Context::from_protocol(Protocol::OpenPgp)?;
    match gpgbin {
        Some(x) => { let _ = ctx.set_engine_path(x.as_str().unwrap().to_string()); },
        None => (),
    }
    let mut b = Vec::new();
    ctx.decrypt(bytes, &mut b)
        .map_err(|e| format!("Decryption failed: {:?}", e))?;
    Ok(b)
}

/// Unpack tar data and write the folder to disk.
async fn unpack_tar(bytes: &[u8], dest: &Path)
-> Result<(), std::io::Error> {
    let dec = GzDecoder::new(bytes);
    let mut tar = Archive::new(dec);
    tar.unpack(dest)?;
    Ok(())
}

/// Create a compressed and archived sync folder.
async fn create_tar(source: &Path) -> Result<Vec<u8>, std::io::Error> {
    let enc= GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = Builder::new(enc);
    tar.append_dir_all("", source)?;
    let enc_data: GzEncoder<Vec<u8>> = tar.into_inner()?;
    let comp_vec: Vec<u8> = enc_data.finish()?;
    Ok(comp_vec)
}

/// Encrypt data with the given GPG key.
async fn encrypt(bytes: &[u8], gpgid: &str, gpgbin: &Option<Value>, symmetric: bool)
-> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut ctx = Context::from_protocol(Protocol::OpenPgp)?;
    match gpgbin {
        Some(x) => { let _ = ctx.set_engine_path(x
                          .as_str()
                          .unwrap()
                          .to_string());
                   },
        None => (),
    }
    ctx.set_armor(true);
    let key = ctx.get_key(gpgid)?;
    let mut b = Vec::new();
    if symmetric {
        ctx.encrypt_symmetric(bytes, &mut b)?;
    } else {
        ctx.encrypt([&key], bytes, &mut b)?;
    }
    Ok(b)
}


/// Write bytes to a file on the remote system. 
async fn write_remote_file(s: &mut Session, bytes: &[u8], dest: &str)
-> Result<(), Box<dyn std::error::Error>> {
    let mut cmd = s.command("dd")
            .stdin(Stdio::piped())
            .arg("status=progress")
            .arg(format!("of={}", dest))
            .spawn()?;
    let stdin = cmd
        .stdin()
        .as_mut()
        .ok_or("Remote: dd: Unable to pipe to stdin")?;
    stdin.write_all(bytes).await?;
    let status = cmd.wait().await?;
    match status.code() {
        Some(0) => println!("dd: {} to remote host", &dest),
        None => println!("Warning: dd {} on remote host: no exit code", &dest),
        _ => println!("Warning: dd: {} to remote host failed", &dest)
    }
    Ok(())
}

/// Test whether a file exists on the remote filesystem.
async fn confirm_remote_exists(s: &mut Session, file: &str)
    -> Result<bool, Box<dyn std::error::Error>> {
    let cmd = s.command("test")
            .arg("-f")
            .arg(file)
            .status()
            .await?;
    match cmd.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        Some(_) => Err(format!("{:?}", &cmd).into()),
        None    => Err("Remote: 'test': no exit code".into()),
    }
}

/// Alternative to write_remote_file() for writing files over SSH, which
/// uses dd. This doesn't utilize the openssh Rust bindings but just
/// calls scp directly, which provides some progress information.
/// Only needs to be used if the other method isn't working or the 
/// transfer time is long enough that it warrants progress updates.
async fn scp_write(bytes: &[u8], dest: &str, sshaddr: &str) -> std::io::Result<()> {
    let mut f = std::fs::File::create(dest)?;
    f.write_all(bytes)?;
    let cmd = std::process::Command::new("rsync")
        .arg("--progress")
        .arg(dest)
        .arg(format!("{}:{}", sshaddr, dest))
        .status()?;
    println!("{:?}", cmd);
    std::fs::remove_file(dest)?;
    println!("Wrote using scp.");
    Ok(())
}

/// Download the remote archive and unpack it to a location on disk.
/// The temp argument determines whether the archive is being unpacked
/// to a temporary directory or to the synced folder itself (e.g., if
/// the folder is being pulled locally for the first time).
async fn pull_remote(s: &mut Session, cfg: &Config, temp: bool)
-> Result<(), Box<dyn std::error::Error>> {
    println!("Pulling from remote...");
    let tar = read_remote_file(s, &cfg.tar).await?;
    let tar = decrypt(&tar, &cfg.gpg_bin).await?;
    let dest = match temp {
        true  => &cfg.temp,
        false => &cfg.dir,
    };
    unpack_tar(&tar, dest).await?;
    Ok(())
}

/// Write archive of the sync directory and its hash to the remote file system.
async fn push_remote(s: &mut Session, cfg: &Config, args: &Args)
-> Result<(), Box<dyn std::error::Error>> {
    let hash = hash_metadata(&cfg.dir).await;
    let tar = create_tar(&cfg.dir).await?;
    let tar = encrypt(&tar, &cfg.gpg_id, &cfg.gpg_bin, cfg.symmetric).await?;
    if args.scpwrite {
        scp_write(&tar, &cfg.tar, &cfg.sshaddr).await?;
    } else {
        write_remote_file(s, &tar, &cfg.tar).await?;
    }
    match hash {
        Some(x) => {
            let bytes: Vec<u8> = x.to_be_bytes().to_vec();
            write_remote_file(s, &bytes, &cfg.tar_hash).await?;
        }
        None => println!("Error hashing the sync folder."),
    }
    Ok(())
}


/// Returns the path specified by the $HOME environmental variable, if set.
async fn home_from_env() -> Option<PathBuf> {
    let home_env = var("HOME").ok()?;
    Some(PathBuf::from(home_env))
}

#[derive(Parser, Debug)]
#[clap(about, version, author)]
struct Args {
    #[clap(help("The configuration profile to use"))]
    profile: String,
    #[clap(short('p'), long("push"), takes_value(false), conflicts_with("pull"),
           help("Copy local to remote without syncing, overwriting remote if it exists"))]
    push: bool,
    #[clap(short('P'), long("pull"), takes_value(false), conflicts_with("push"),
           help("Copy remote to local without syncing, overwriting local if it exists"))]
    pull: bool,
    #[clap(short('y'), long("assume-yes"), takes_value(false),
           help("Assume yes to all prompts and run with no interaction"))]
    assumeyes: bool,
    #[clap(short('s'), long("scp-write"), takes_value(false),
           help("Write remote files using scp (shows progress)"))]
    scpwrite: bool,
}

/// Ask for user confirmation, return true if confirmation recieved or false if not.
fn user_confirm(prompt: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true
    }
    println!("{}", prompt);
    let mut inpt = String::new();
    stdin().read_line(&mut inpt).expect("Failed to read line");
    matches!(inpt.trim(), "y" | "Y" | "yes")
}

/// Hash the metadata of the contents of a directory.
async fn hash_metadata(path: &Path) -> Option<u64> {
    let mut hash = XxHash64::with_seed(42);
    for e in WalkDir::new(path)
        .sort_by_file_name()
        .into_iter().filter_map(|e| e.ok()) {
        if ! e.path().is_file() {
            continue
        }
        let meta = e.metadata().ok()?;
        e.path().file_name()?.hash(&mut hash);
        meta.len().hash(&mut hash);
    }
    Some(hash.finish())
}

async fn run_mist(home: &Path, cfg: &Config, args: &Args, s: &mut Session)
-> Result<(), Box<dyn std::error::Error>> {
    if args.push {
        let tar_is = confirm_remote_exists(s, &cfg.tar).await.unwrap();
        if tar_is && ! user_confirm("Remote storage exists: overwrite?",
            args.assumeyes) {
            return Ok(())
        }
        push_remote(s, cfg, args).await?;
    } else if args.pull {
        let dir_is = confirm_local_exists(home, &cfg.dir).await?;
        if dir_is && ! user_confirm("Local directory exists: overwrite?",
            args.assumeyes) {
            return Ok(())
        }
        pull_remote(s, cfg, false).await?;
    } else {
        let far_hash = read_remote_file(s, &cfg.tar_hash).await.ok();
        let near_hash = hash_metadata(&cfg.dir).await;
        if far_hash.is_some() && near_hash.is_some() {
            let near_hash = near_hash
                .unwrap()
                .to_be_bytes();
            let far_hash = far_hash
                .unwrap();
            if far_hash == near_hash {
                println!("Already up to date");
                return Ok(())
            }
        }
        pull_remote(s, cfg, true).await?;
        match unison(&cfg.dir, &cfg.temp, args.assumeyes).await? {
            true  => (),
            false => {
                let q = "Unison may have produced an error. Transfer to remote anyway?";
                if ! user_confirm(q, args.assumeyes) {
                    return Ok(())
                }
            }
        }
        push_remote(s, cfg, args).await?;
        match remove_dir_all(&cfg.temp) {
            Ok(()) => println!("Deleting temporary directory"),
            Err(e) => println!("Error deleting temporary directory: {}", e),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let home = home_from_env().await.expect("$HOME variable not set.");
    let cfg = load_configuration(&home, &args.profile)
        .await
        .expect("Missing configuration parameters");

    let mut s = SessionBuilder::default()
        .known_hosts_check(KnownHosts::Strict) 
        .control_directory("/tmp")
        .connect(&cfg.sshaddr).await?;

    run_mist(&home, &cfg, &args, &mut s).await?;

    s.close().await?;

    Ok(())
}
