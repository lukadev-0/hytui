use hytui::http_client;

fn main() {
    async_io::block_on(run());
}

async fn run() {
    let url = std::env::args()
        .nth(1)
        .expect("should have an arg")
        .parse()
        .unwrap();

    let res = http_client::request(&url).await.unwrap();

    println!("{res}");
}
