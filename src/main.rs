mod config;
mod db;
mod discord;
mod mesh;

use crate::discord::commands;
use crate::mesh::{DiscordToMesh, MeshToDiscord};
use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage, CreateThread,
};
use serenity::model::application::{Command, Interaction};
use serenity::model::channel::ChannelType;
use serenity::model::gateway::{GatewayIntents, Ready};
use serenity::model::id::ChannelId;
use serenity::prelude::*;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, info};

struct Handler {
    db_pool: SqlitePool,
    mesh_tx: mpsc::Sender<DiscordToMesh>,
}

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            let content = match command.data.name.as_str() {
                "setchannel" => commands::handle_setchannel(&ctx, &command, &self.db_pool).await,
                "opendm" => commands::handle_opendm(&ctx, &command, &self.db_pool).await,
                "post" => commands::handle_post(&ctx, &command, &self.db_pool, &self.mesh_tx).await,
                _ => "not implemented :(".to_string(),
            };

            let data = CreateInteractionResponseMessage::new().content(content);
            let builder = CreateInteractionResponse::Message(data);
            if let Err(why) = command.create_response(&ctx.http, builder).await {
                error!("Cannot respond to slash command: {why}");
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("{} is connected!", ready.user.name);

        let _ = Command::set_global_commands(
            &ctx.http,
            vec![
                CreateCommand::new("setchannel")
                    .description("Set the primary channel for mesh messages"),
                CreateCommand::new("opendm")
                    .description("Open a DM thread with a node")
                    .add_option(
                        serenity::builder::CreateCommandOption::new(
                            serenity::model::application::CommandOptionType::String,
                            "node_id",
                            "The Meshtastic Node ID (e.g. !1234abcd)",
                        )
                        .required(true),
                    ),
                CreateCommand::new("post")
                    .description("Send a message to the mesh")
                    .add_option(
                        serenity::builder::CreateCommandOption::new(
                            serenity::model::application::CommandOptionType::String,
                            "message",
                            "The message to send",
                        )
                        .required(true),
                    )
                    .add_option(
                        serenity::builder::CreateCommandOption::new(
                            serenity::model::application::CommandOptionType::String,
                            "destination",
                            "The Meshtastic Node ID (optional if inside a DM thread)",
                        )
                        .required(false),
                    ),
            ],
        )
        .await;
    }
}

pub async fn run_discord_outgoing_loop(
    http: Arc<serenity::http::Http>,
    db_pool: SqlitePool,
    mut rx: mpsc::Receiver<MeshToDiscord>,
) {
    while let Some(msg) = rx.recv().await {
        match msg {
            MeshToDiscord::IncomingMessage {
                sender,
                text,
                is_dm,
            } => {
                let rows = sqlx::query("SELECT channel_id FROM guild_channels")
                    .fetch_all(&db_pool)
                    .await;

                if let Ok(channels) = rows {
                    for record in channels {
                        let cid_str: String = record.get("channel_id");
                        let cid: u64 = cid_str.parse().unwrap();
                        let channel_id = ChannelId::new(cid);

                        if is_dm {
                            let thread_name = format!("Mesh DM: {}", sender);
                            let builder =
                                CreateThread::new(thread_name).kind(ChannelType::PublicThread);

                            if let Ok(thread) = channel_id.create_thread(&http, builder).await {
                                let formatted = format!("**[Mesh DM] {}**: {}", sender, text);
                                let _ = thread.say(&http, formatted).await;

                                let thread_id = thread.id.get().to_string();
                                if let Ok(channel_obj) = channel_id.to_channel(&http).await {
                                    if let Some(guild_ch) = channel_obj.guild() {
                                        let guild_id = guild_ch.guild_id.to_string();
                                        let res = sqlx::query("INSERT INTO mesh_threads (thread_id, guild_id, destination) VALUES (?, ?, ?)")
                                            .bind(&thread_id)
                                            .bind(&guild_id)
                                            .bind(&sender)
                                            .execute(&db_pool)
                                            .await;

                                        if let Err(e) = res {
                                            error!("Failed to save new DM thread: {:?}", e);
                                        } else {
                                            commands::update_index_message(
                                                &http, &db_pool, &guild_id,
                                            )
                                            .await;
                                        }
                                    }
                                }
                            }
                        } else {
                            let formatted_msg = format!("**[Mesh] {}**: {}", sender, text);
                            let _ = channel_id.say(&http, formatted_msg).await;
                        }
                    }
                }
            }
        }
    }
}

pub async fn run_mesh_loop(
    port: String,
    mut rx: mpsc::Receiver<DiscordToMesh>,
    _tx: mpsc::Sender<MeshToDiscord>,
) {
    info!("Starting Meshtastic loop on {}", port);

    // TODO wire up tokio_serial here; for example:
    // let mut serial_port = tokio_serial::new(port, 115200).open_native_async().expect("failed to open port");

    loop {
        tokio::select! {
            Some(discord_cmd) = rx.recv() => {
                match discord_cmd {
                    DiscordToMesh::Post { destination, message } => {
                        match destination {
                            Some(dest) => info!("(MOCK MESH) Sending '{}' directly to node/channel {}", message, dest),
                            None => info!("(MOCK MESH) Broadcasting '{}' to primary channel", message),
                        }
                        // TODO construct Meshtastic Protobuf (ToRadio) and write to serial_port.
                    }
                    DiscordToMesh::Reboot => {
                        info!("(MOCK MESH) Rebooting node...");
                    }
                }
            }

            // TODO Add a branch here to read from serial port e.g.
            // Ok(bytes_read) = serial_port.read(&mut buffer) => {
            //     Parse protobuf (FromRadio).
            //     let sender = parsed.sender;
            //     let text = parsed.text;
            //     let is_dm = parsed.is_dm;
            //     let _ = _tx.send(MeshToDiscord::IncomingMessage { sender, text, is_dm }).await;
            // }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let cfg = config::load_config();
    let db_pool = db::init_db(&cfg.database.url).await;

    // Discord -> Mesh channel
    let (discord_to_mesh_tx, discord_to_mesh_rx) = mpsc::channel::<DiscordToMesh>(100);

    // Mesh -> Discord channel
    let (mesh_to_discord_tx, mesh_to_discord_rx) = mpsc::channel::<MeshToDiscord>(100);

    let handler = Handler {
        db_pool: db_pool.clone(),
        mesh_tx: discord_to_mesh_tx.clone(),
    };

    let mut client = Client::builder(&cfg.bot.token, GatewayIntents::empty())
        .event_handler(handler)
        .await
        .expect("Failed to create the client.");

    let http = client.http.clone();
    let db_clone = db_pool.clone();

    tokio::spawn(async move {
        run_discord_outgoing_loop(http, db_clone, mesh_to_discord_rx).await;
    });

    tokio::spawn(async move {
        run_mesh_loop(
            cfg.meshtastic.serial_port,
            discord_to_mesh_rx,
            mesh_to_discord_tx,
        )
        .await;
    });

    if let Err(why) = client.start().await {
        error!("Client error: {why:?}");
    }
}
