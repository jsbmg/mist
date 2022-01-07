# Mist

Mist is a directory syncer with GPG-encrypted remote storage. It is intended to be used as a secure way to share directories between computers that have access to a remote SSH server, such as Rsync.net, without leaving sensitive information such as documents or passwords in the open. It utilizies [GPGME](https://www.gnupg.org/software/gpgme/index.html) and [Unison](https://www.cis.upenn.edu/~bcpierce/unison/). 

**Advantages**:
* Simple to use and secure
* No need to trust a traditional cloud provider
* Easy to install
* Fast for small directories
* No software needs to be installed remotely (except for standard Unix tools)

**Disadvantages**:
* Not practical for large directories
* Incremental syncing over the net not supported

## Installation

Ensure the following programs are installed:
* GPGME
* Unison

```
git clone https://github.com/jsbmg/mist
cd mist && cargo install --path .
```

## Configuration
 
Mist reads a configuration file, which can be placed in the following locations:

`$HOME/.config/mist/mist.toml`

`$HOME/.config/mist.toml`

`$HOME/.mist.toml`

Each `[section]` of the configuration defines a *profile*, under which a few variables are defined for that profile (such as the directory, and the ssh address, etc.). Multiple profiles can be configured for different directories. See `mist.toml` in the repository for an example. 

## Usage

Download the directory to the remote filesystem:
```
mist [PROFILE] --push
```
"Install" the directory to a new local machine:
```
mist [PROFILE] --pull
```
Sync the directory contents between the local and remote filesystems:
```
mist [PROFILE]
```
