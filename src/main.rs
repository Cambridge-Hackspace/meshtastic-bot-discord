use serde::Deserialize;
use serenity::async_trait;
use serenity::builder::{
    CreateCommand, CreateInteractionResponse, CreateInteractionResponseMessage,
};
use serenity::model::application::Interaction;
use serenity::model::channel::Reaction;
use serenity::model::gateway::Ready;
use serenity::prelude::*;
use std::fs;
use tracing::{error, info, warn};

#[derive(Deserialize)]
struct Config {
    bot: BotConfig,
}

#[derive(Deserialize)]
struct BotConfig {
    token: String,
}

struct Handler;

#[async_trait]
impl EventHandler for Handler {
    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        if let Interaction::Command(command) = interaction {
            info!(
                "Received command '{}' from user '{}'",
                command.data.name, command.user.name
            );

            if "ping" == command.data.name.as_str() {
                let response_data = CreateInteractionResponseMessage::new().content("Pong!");
                let builder = CreateInteractionResponse::Message(response_data);

                if let Err(why) = command.create_response(&ctx.http, builder).await {
                    error!("Cannot respond to slash command: {why:?}");
                }
            }
        }
    }

    async fn ready(&self, ctx: Context, ready: Ready) {
        info!("{} is connected and ready!", ready.user.name);

        let ping_command = CreateCommand::new("ping").description("A simple ping command");

        if let Err(why) =
            serenity::model::application::Command::create_global_command(&ctx.http, ping_command)
                .await
        {
            error!("Failed to register slash commands: {why:?}");
        } else {
            info!("Successfully registered global slash commands.");
        }
    }

    async fn reaction_add(&self, _: Context, reaction: Reaction) {
        if let Some(member) = reaction.member {
            info!(
                "User {} reacted with {} to message no. {}.",
                member.user.name, reaction.emoji, reaction.message_id
            );
        } else {
            warn!(
                "An unknown user reacted with {} to message no. {}.",
                reaction.emoji, reaction.message_id
            );
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Initializing the bot...");

    let config_contents = fs::read_to_string("config.toml")
        .expect("Failed to read config.toml. Make sure it exists in the project root.");

    let config: Config = toml::from_str(&config_contents)
        .expect("Failed to parse config.toml. Please verify your syntax.");

    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILD_MESSAGE_REACTIONS;

    let mut client = Client::builder(&config.bot.token, intents)
        .event_handler(Handler)
        .await
        .expect("Error creating the Discord client.");

    if let Err(why) = client.start().await {
        error!("Client error: {why:?}");
    }
}
