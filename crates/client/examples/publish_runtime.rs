use xr_corpus_client::{
    CorpusClient,
    protocol::{
        CORPUS_LANGUAGE_ORDER, CORPUS_SCHEMA, CorpusActivation, CorpusDefinition, CorpusTerm,
        PublishProviderRequest,
    },
};

fn multilingual_term(english: &str) -> CorpusTerm {
    let mut values = vec![String::new(); CORPUS_LANGUAGE_ORDER.len()];
    values[1] = english.to_owned();
    CorpusTerm::from_ordered(values).expect("valid fixed-order term")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = CorpusClient::connect("http://127.0.0.1:7766").await?;
    let response = client
        .publish_provider(
            "example-game",
            &PublishProviderRequest {
                ttl_seconds: 30,
                corpora: vec![CorpusDefinition {
                    schema: CORPUS_SCHEMA.into(),
                    id: "runtime.example-game.room".into(),
                    domain: "games".into(),
                    subdomain: "example-game".into(),
                    title: "Current room".into(),
                    priority: 100,
                    activation: CorpusActivation::Always,
                    triggers: Vec::new(),
                    trigger_aliases: Vec::new(),
                    activation_context: Vec::new(),
                    terms: vec![multilingual_term("Player One")],
                }],
            },
        )
        .await?;
    println!("published {} corpus", response.corpus_count);
    Ok(())
}
