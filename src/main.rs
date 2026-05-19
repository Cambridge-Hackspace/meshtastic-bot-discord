mod config;
mod db;
mod discord;
mod mesh;

use crate::discord::commands;
use crate::mesh::{DiscordToMesh, MeshToDiscord};
use bytes::{Buf, BytesMut};
use prost::Message as ProstMessage;
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
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;
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
                "reboot" => {
                    let _ = self.mesh_tx.send(DiscordToMesh::Reboot).await;
                    "Reboot command sent to the local Meshtastic node!".to_string()
                }
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
                CreateCommand::new("reboot")
                    .description("Restart the locally attached Meshtastic node"),
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

async fn process_serial_buffer(buf: &mut BytesMut, tx: &mpsc::Sender<MeshToDiscord>) {
    while buf.len() >= 4 {
        // look for Meshtastic magic header (0x94, 0xc3)
        let Some(pos) = buf.windows(2).position(|w| w == [0x94, 0xc3]) else {
            // magic bytes not found, but keep the last byte just in case
            let keep = if buf.last() == Some(&0x94) { 1 } else { 0 };
            let drop_len = buf.len() - keep;
            buf.advance(drop_len);
            return;
        };

        if pos > 0 {
            buf.advance(pos); // drop pre-magic garbage
        }

        if buf.len() < 4 {
            return;
        }

        let len = ((buf[2] as usize) << 8) | (buf[3] as usize);
        if buf.len() < 4 + len {
            return;
        } // wait for rest of packet

        // meat's back on the menu, boys!
        let frame = buf.split_to(4 + len);
        let payload = &frame[4..];

        let Ok(from_radio) = meshtastic::protobufs::FromRadio::decode(payload) else {
            continue;
        };
        let Some(meshtastic::protobufs::from_radio::PayloadVariant::Packet(packet)) =
            from_radio.payload_variant
        else {
            continue;
        };
        let Some(meshtastic::protobufs::mesh_packet::PayloadVariant::Decoded(data)) =
            packet.payload_variant
        else {
            continue;
        };

        // process text only
        if data.portnum == meshtastic::protobufs::PortNum::TextMessageApp as i32 {
            if let Ok(text) = String::from_utf8(data.payload) {
                let sender = format!("!{:08x}", packet.from);
                let is_dm = packet.to != 0xffffffff;
                let _ = tx
                    .send(MeshToDiscord::IncomingMessage {
                        sender,
                        text,
                        is_dm,
                    })
                    .await;
            }
        }
    }
}

pub async fn run_mesh_loop(
    port: String,
    mut rx: mpsc::Receiver<DiscordToMesh>,
    tx: mpsc::Sender<MeshToDiscord>,
) {
    info!("Starting Meshtastic loop on {}", port);

    let mut serial_port = tokio_serial::new(&port, 115200)
        .open_native_async()
        .unwrap_or_else(|e| panic!("Failed to open port {}: {}", port, e));

    let mut buf = BytesMut::new();

    loop {
        tokio::select! {
            Some(discord_cmd) = rx.recv() => {
                match discord_cmd {
                    DiscordToMesh::Post { destination, message } => {
                        info!("Sending message to mesh: {:?}", destination);

                        // parse destination ID (broadcast = 0xffffffff)
                        let to_node = if let Some(dest) = destination {
                            if let Some(hex_str) = dest.strip_prefix('!') {
                                u32::from_str_radix(hex_str, 16).unwrap_or(0xffffffff)
                            } else {
                                0xffffffff
                            }
                        } else {
                            0xffffffff
                        };

                        // construct protobufs
                        let data = meshtastic::protobufs::Data {
                            portnum: meshtastic::protobufs::PortNum::TextMessageApp as i32,
                            payload: message.into_bytes(),
                            want_response: false,
                            ..Default::default()
                        };

                        let packet = meshtastic::protobufs::MeshPacket {
                            from: 0,
                            to: to_node,
                            payload_variant: Some(meshtastic::protobufs::mesh_packet::PayloadVariant::Decoded(data)),
                            id: 0,
                            want_ack: false,
                            ..Default::default()
                        };

                        let to_radio = meshtastic::protobufs::ToRadio {
                            payload_variant: Some(meshtastic::protobufs::to_radio::PayloadVariant::Packet(packet)),
                        };

                        let payload_bytes = to_radio.encode_to_vec();
                        let len = payload_bytes.len();

                        // 0x94, 0xc3, MSB, LSB, payload
                        let mut out_buf = vec![0x94, 0xc3, (len >> 8) as u8, (len & 0xff) as u8];
                        out_buf.extend(payload_bytes);

                        if let Err(e) = serial_port.write_all(&out_buf).await {
                            error!("Failed to write to serial port: {}", e);
                        }
                    }

                    DiscordToMesh::Reboot => {
                        info!("Sending reboot command to local node...");

                        let admin_msg = meshtastic::protobufs::AdminMessage {
                            payload_variant: Some(meshtastic::protobufs::admin_message::PayloadVariant::RebootSeconds(3)),
                            ..Default::default()
                        };

                        let data = meshtastic::protobufs::Data {
                            portnum: meshtastic::protobufs::PortNum::AdminApp as i32,
                            payload: admin_msg.encode_to_vec(),
                            ..Default::default()
                        };

                        let packet = meshtastic::protobufs::MeshPacket {
                            to: 0,
                            payload_variant: Some(meshtastic::protobufs::mesh_packet::PayloadVariant::Decoded(data)),
                            ..Default::default()
                        };

                        let to_radio = meshtastic::protobufs::ToRadio {
                            payload_variant: Some(meshtastic::protobufs::to_radio::PayloadVariant::Packet(packet)),
                        };

                        let payload_bytes = to_radio.encode_to_vec();
                        let len = payload_bytes.len();
                        let mut out_buf = vec![0x94, 0xc3, (len >> 8) as u8, (len & 0xff) as u8];
                        out_buf.extend(payload_bytes);

                        if let Err(e) = serial_port.write_all(&out_buf).await {
                            error!("Failed to write reboot cmd to serial port: {}", e);
                        }
                    }
                }
            }

            res = serial_port.read_buf(&mut buf) => {
                match res {
                    Ok(0) => {
                        error!("Serial port closed unexpectedly.");
                        break;
                    }
                    Ok(_) => {
                        process_serial_buffer(&mut buf, &tx).await;
                    }
                    Err(e) => {
                        error!("Serial read error: {}", e);
                        break;
                    }
                }
            }
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
