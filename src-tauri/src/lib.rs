mod app;
mod ask_ai;
mod commands;
mod database;
mod detection;
mod exports;
mod helper;
mod recording;
mod state;
mod summarization;
mod transcription;
mod types;
mod webhooks;

pub fn run() {
    app::run();
}
