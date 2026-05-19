use crate::mesh::DiscordToMesh;
use serenity::builder::{
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateThread,
    EditMessage,
};
use serenity::model::application::CommandInteraction;
use serenity::model::channel::ChannelType;
use serenity::model::id::{ChannelId, MessageId};
use serenity::prelude::*;
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::error;

pub async fn update_index_message(http: &Arc<Http>, db_pool: &SqlitePool, guild_id: &str) {
    let row =
        sqlx::query("SELECT channel_id, index_message_id FROM guild_channels WHERE guild_id = ?")
            .bind(guild_id)
            .fetch_optional(db_pool)
            .await;

    if let Ok(Some(r)) = row {
        let channel_id_str: String = r.get("channel_id");
        let index_msg_id_str: Option<String> = r.try_get("index_message_id").ok().flatten();

        if let (Ok(cid), Some(mid_str)) = (channel_id_str.parse::<u64>(), index_msg_id_str) {
            if let Ok(mid) = mid_str.parse::<u64>() {
                let mut content = String::from("** :satellite: Meshtastic Active Threads**\n\n");

                let threads = sqlx::query(
                    "SELECT thread_id, destination FROM mesh_threads WHERE guild_id = ?",
                )
                .bind(guild_id)
                .fetch_all(db_pool)
                .await
                .unwrap_or_default();

                if threads.is_empty() {
                    content.push_str("*No active DMs or secondary channels.");
                } else {
                    for t in threads {
                        let tid: String = t.get("thread_id");
                        let dest: String = t.get("destination");
                        content.push_str(&format!(":arrow_right: <#{}> - `{}`\n", tid, dest));
                    }
                }

                let builder = EditMessage::new().content(content);
                let _ = ChannelId::new(cid)
                    .edit_message(http, MessageId::new(mid), builder)
                    .await;
            }
        }
    }
}

pub async fn handle_setchannel(
    _ctx: &Context,
    command: &CommandInteraction,
    db_pool: &SqlitePool,
) -> String {
    let guild_id = command.guild_id.unwrap().get().to_string();
    let channel_id = command.channel_id.get().to_string();

    // channel cleanup
    if let Ok(Some(r)) =
        sqlx::query("SELECT channel_id, index_message_id FROM guild_channels WHERE guild_id = ?")
            .bind(&guild_id)
            .fetch_optional(db_pool)
            .await
    {
        let old_cid_str: String = r.get("channel_id");
        if old_cid_str != channel_id {
            if let Ok(old_cid) = old_cid_str.parse::<u64>() {
                let old_channel = ChannelId::new(old_cid);

                // delete old index message
                if let Ok(Some(old_mid_str)) = r.try_get::<Option<String>, _>("index_message_id") {
                    if let Ok(old_mid) = old_mid_str.parse::<u64>() {
                        let _ = old_channel
                            .delete_message(&_ctx.http, MessageId::new(old_mid))
                            .await;
                    }
                }

                // delete old threads
                let old_threads =
                    sqlx::query("SELECT thread_id FROM message_threads where guild_id = ?")
                        .bind(&guild_id)
                        .fetch_all(db_pool)
                        .await
                        .unwrap_or_default();
                for t in old_threads {
                    let tid_str: String = t.get("thread_id");
                    if let Ok(tid) = tid_str.parse::<u64>() {
                        let _ = ChannelId::new(tid).delete(&_ctx.http).await;
                    }
                }
                let _ = sqlx::query("DELETE FROM mesh_threads WHERE guild_id = ?")
                    .bind(&guild_id)
                    .execute(db_pool)
                    .await;
            }
        }
    }

    // create new pinned index message
    let builder = CreateMessage::new()
        .content("** :satellite: Meshtastic Active Threads**\n\n*Initializing...*");
    let index_msg_id = match command.channel_id.send_message(&_ctx.http, builder).await {
        Ok(msg) => {
            let _ = msg.pin(&_ctx.http).await;
            Some(msg.id.get().to_string())
        }
        Err(_) => None,
    };

    let res = sqlx::query("INSERT INTO guild_channels (guild_id, channel_id, index_message_id) VALUES (?, ?, ?) ON CONFLICT(guild_id) DO UPDATE SET channel_id = excluded.channel_id, index_message_id = excluded.index_message_id")
        .bind(&guild_id)
        .bind(&channel_id)
        .bind(index_msg_id)
        .execute(db_pool)
        .await;

    update_index_message(&_ctx.http, db_pool, &guild_id).await;

    match res {
        Ok(_) => "Okay! I'll use this channel for Mesh messages from now on.".to_string(),
        Err(e) => {
            error!("Database malfunction: {:?}", e);
            "Oops! I couldn't save your preferences for some reason.".to_string()
        }
    }
}

pub async fn handle_opendm(
    ctx: &Context,
    command: &CommandInteraction,
    db_pool: &SqlitePool,
) -> String {
    let guild_id = command.guild_id.unwrap().get().to_string();
    let node_id = command
        .data
        .options
        .first()
        .and_then(|opt| opt.value.as_str())
        .unwrap_or("")
        .to_string();

    if node_id.is_empty() {
        return "Please provide a valid node ID.".to_string();
    }

    let existing =
        sqlx::query("SELECT thread_id FROM mesh_threads WHERE guild_id = ? AND destination = ?")
            .bind(&guild_id)
            .bind(&node_id)
            .fetch_optional(db_pool)
            .await;

    if let Ok(Some(r)) = existing {
        let tid: String = r.get("thread_id");
        return format!("A thread for `{}` already exists: <#{}>", node_id, tid);
    }

    let primary_ch = sqlx::query("SELECT channel_id FROM guild_channels WHERE guild_id = ?")
        .bind(&guild_id)
        .fetch_optional(db_pool)
        .await;

    let parent_channel = match primary_ch {
        Ok(Some(r)) => {
            let cid_str: String = r.get("channel_id");
            ChannelId::new(cid_str.parse().unwrap())
        }
        _ => return "Please run `/setchannel` first to designate a primary channel.".to_string(),
    };

    let thread_name = format!("Mesh DM: {}", node_id);
    let builder = CreateThread::new(thread_name).kind(ChannelType::PublicThread);

    match parent_channel.create_thread(&ctx.http, builder).await {
        Ok(thread) => {
            let thread_id = thread.id.get().to_string();
            let res = sqlx::query(
                "INSERT INTO mesh_threads (thread_id, guild_id, destination) VALUES (?, ?, ?)",
            )
            .bind(&thread_id)
            .bind(&guild_id)
            .bind(&node_id)
            .execute(db_pool)
            .await;

            match res {
                Ok(_) => {
                    update_index_message(&ctx.http, db_pool, &guild_id).await;
                    format!("Opened a DM thread for `{}`: <#{}>", node_id, thread_id)
                }
                Err(e) => {
                    error!("Database error: {:?}", e);
                    "Created the thread, but failed to link it in the database.".to_string()
                }
            }
        }
        Err(e) => {
            error!("Failed to create thread: {:?}", e);
            "Failed to create thread. Please make sure I have permission to create public threads!"
                .to_string()
        }
    }
}

pub async fn handle_post(
    _ctx: &Context,
    command: &CommandInteraction,
    db_pool: &SqlitePool,
    mesh_tx: &mpsc::Sender<DiscordToMesh>,
) -> String {
    let message = command
        .data
        .options
        .iter()
        .find(|opt| opt.name == "message")
        .and_then(|opt| opt.value.as_str())
        .unwrap_or("")
        .to_string();

    if message.is_empty() {
        return "Cannot send an empty message.".to_string();
    }

    let mut dest = command
        .data
        .options
        .iter()
        .find(|opt| opt.name == "destination")
        .and_then(|opt| opt.value.as_str())
        .map(|s| s.to_string());

    if dest.is_none() {
        let thread_id = command.channel_id.get().to_string();
        let row = sqlx::query("SELECT destination FROM mesh_threads WHERE thread_id = ?")
            .bind(&thread_id)
            .fetch_optional(db_pool)
            .await;

        if let Ok(Some(r)) = row {
            dest = Some(r.get("destination"));
        } else {
            let guild_id = command.guild_id.unwrap().get().to_string();
            let row2 = sqlx::query(
                "SELECT channel_id FROM guild_channels WHERE guild_id = ? AND channel_id = ?",
            )
            .bind(&guild_id)
            .bind(&thread_id)
            .fetch_optional(db_pool)
            .await;

            if let Ok(Some(_)) = row2 {
                let _ = mesh_tx
                    .send(DiscordToMesh::Post {
                        destination: None,
                        message,
                    })
                    .await;
                return "Queued message for broadcast on primary channel.".to_string();
            }
        }
    }

    match dest {
        Some(destination) => {
            let _ = mesh_tx
                .send(DiscordToMesh::Post {
                    destination: Some(destination.clone()),
                    message,
                })
                .await;
            format!("Queued message for `{}`.", destination)
        }
        None => {
            "Could not determine destination. Please provide a `destination`, use this command inside a DM thread, or use it in the primary mesh channel.".to_string()
        }
    }
}
