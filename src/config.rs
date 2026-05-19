use serde::Deserialize;
use std::fs;

#[derive(Deserialize, Clone)]
pub struct Config {
    pub bot: BotConfig,
    pub database: DbConfig,
    pub meshtastic: MeshConfig,
}

#[derive(Deserialize, Clone)]
pub struct BotConfig {
    pub token: String,
}

#[derive(Deserialize, Clone)]
pub struct DbConfig {
    pub url: String,
}

#[derive(Deserialize, Clone)]
pub struct MeshConfig {
    pub serial_port: String,
}

pub fn load_config() -> Config {
    let contents = fs::read_to_string("config.toml").expect("Failed to read config.toml");
    toml::from_str(&contents).expect("Failed to parse the config file.")
}
