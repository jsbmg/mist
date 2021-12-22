use std::path::PathBuf;
use std::fs::{ read_to_string };


use toml::Value;

pub struct Config {
    pub dir: PathBuf,
    pub sshaddr: String,
    pub temp: PathBuf,
    pub gpg_id: String,
    pub tar: String,
    pub tar_hash: String,
    pub gpg_bin: Option<Value>,
}

/// Load the configuration file and unpack its values.
/// 
/// The following locations are checked:
/// 1. $HOME/.config/mist/mist.toml
/// 2. $HOME/.config/mist.toml
/// 3. $HOME/.mist.toml 
///
/// The configuration file has the following parameters. 
/// [<profile-name>]            
/// folder = "/path/to/sync/folder"  (folder to sync)
/// ssh_address = "user@host" (remote ssh address to sync with)
/// gpg_id = "youremail@yourprovider.com" (gpg id to encrypt with)
/// temp_folder    = "/tmp/sync-folder" (temp folder location)
///
/// Note that multiple profiles are allowed and the profile to use at runtime 
/// is specified as a required argument.
pub async fn load_configuration(home: &PathBuf, profile: &str) 
-> Result<Config, Box<dyn std::error::Error>> {
    let toml = std::fs::read_to_string(home.join(".config/mist/mist.toml"))
        .or(read_to_string(home.join(".config/mist.toml")))
        .or(read_to_string(home.join("mist.toml")))
        .expect("No configuration file found.");

    let values: Value = toml::from_str(&toml)?;  

    // Check the configuration file is populated correctly 
    let cfg = match values.get(profile) {
        Some(x) => x,
        None => {
            println!("Configuration error: profile [{}] not found", &profile);
            panic!();
        }
    };

    for x in ["folder", "ssh_address", "gpg_id", "temp_folder"] {
        match cfg.get(x) {
            Some(_) => (),
            None => { 
                println!("Configuration error: profile [{}] missing '{}' entry", 
                         &profile, x);
            }
        }  
    };

    let dir = &cfg
        .get("folder")
        .unwrap()
        .as_str()
        .ok_or("Can't parse 'sync folder' value as str")?;
    let sshaddr = &cfg
        .get("ssh_address")
        .unwrap()
        .as_str()
        .ok_or("Can't parse 'remote_address' value as str")?;
    let gpgid = &cfg
        .get("gpg_id")
        .unwrap()
        .as_str()
        .ok_or("Can't parse 'gpg_recipient' value as str")?;
    let tmp = &cfg
        .get("temp_folder")
        .unwrap()
        .as_str()
        .ok_or("Can't parse 'temp_folder' value as str")?;
    
    let tar = PathBuf::from(&tmp)
        .with_extension("tar.gz.gpg");
    let tar_hash = PathBuf::from(&tar)
        .with_extension("gpg.xxhash");
    let tar_hash = &tar_hash
        .file_name().unwrap()
        .to_str().unwrap();
    let tar = &tar
        .file_name().unwrap()
        .to_str().unwrap();

    let gpgbin = cfg
        .get("gpg_program").to_owned(); 

    let config = Config {
        dir: PathBuf::from(dir),
        sshaddr: sshaddr.to_string(),
        gpg_id: gpgid.to_string(), 
        temp: PathBuf::from(tmp),
        tar: tar.to_string(),
        tar_hash: tar_hash.to_string(),
        gpg_bin: gpgbin.cloned(),
    };
    Ok(config)
}
