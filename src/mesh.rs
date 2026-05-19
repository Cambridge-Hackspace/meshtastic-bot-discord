#[derive(Debug, Clone)]
pub enum DiscordToMesh {
    Post {
        destination: Option<String>,
        message: String,
    },
    Reboot,
}

#[derive(Debug, Clone)]
pub enum MeshToDiscord {
    IncomingMessage {
        sender: String,
        text: String,
        is_dm: bool,
    },
}
