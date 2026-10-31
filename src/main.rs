use std::sync::Arc;

use anyhow::{Context, Result};
use async_executor::Executor;
use crossterm::event::{Event, KeyCode, KeyModifiers};
use hyper::Uri;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, BorderType, Paragraph, Wrap};
use ratatui_textarea::TextArea;

use hytui::http_client;

fn main() -> Result<()> {
    let ex = Arc::new(Executor::new());
    let (msg_sender, msg_recv) = async_channel::bounded(5);
    let mut app = App::new(ex.clone(), msg_sender.clone());

    std::thread::spawn(move || {
        loop {
            let event = crossterm::event::read()
                .map(Message::Input)
                .context("could not read event from terminal");
            if msg_sender.send_blocking(event).is_err() {
                break;
            }
        }
    });

    async_io::block_on(ex.run(async move {
        let mut terminal = ratatui::init();

        while app.running {
            terminal.draw(|frame| app.render(frame))?;

            match msg_recv.recv().await {
                Ok(Ok(msg)) => app.update(msg),
                Ok(Err(err)) => return Err(err),
                Err(_) => break,
            }
        }

        anyhow::Ok(())
    }))?;

    ratatui::restore();

    Ok(())
}

pub struct App {
    ex: Arc<Executor<'static>>,
    msg_sender: async_channel::Sender<Result<Message>>,
    running: bool,
    url: TextArea<'static>,
    response: String,
}

pub enum Message {
    Input(Event),
    Response(String),
}

impl App {
    fn new(ex: Arc<Executor<'static>>, msg_sender: async_channel::Sender<Result<Message>>) -> Self {
        Self {
            ex,
            msg_sender,
            running: true,
            url: TextArea::default(),
            response: String::new(),
        }
    }

    fn update(&mut self, msg: Message) {
        match msg {
            Message::Input(input) => self.handle_input(input),
            Message::Response(res) => {
                self.response = res;
            }
        }
    }

    fn handle_input(&mut self, event: Event) {
        if let Event::Key(key_event) = event
            && event.is_key_press()
        {
            match key_event.code {
                KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.running = false;
                }
                KeyCode::Enter => {
                    self.handle_submit();
                }
                _ => {
                    self.url.input(key_event);
                }
            }
        }
    }

    fn handle_submit(&mut self) {
        self.response = String::from("Sending...");

        let mut url = self.url.lines().first().cloned().unwrap_or_default();
        if url.is_empty() {
            return;
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            url = format!("https://{url}");
        }

        let url: Uri = match url.parse() {
            Ok(url) => url,
            Err(err) => {
                self.response = err.to_string();
                return;
            }
        };

        self.url.clear();
        self.url.insert_str(url.to_string());

        let msg_sender = self.msg_sender.clone();
        self.ex
            .spawn(async move {
                let res = match http_client::request(&url).await {
                    Ok(res) => res,
                    Err(err) => format!("{err:?}"),
                };
                _ = msg_sender.send(Ok(Message::Response(res))).await;
            })
            .detach();
    }

    fn render(&mut self, frame: &mut Frame) {
        let layout = Layout::new(
            Direction::Vertical,
            [Constraint::Length(3), Constraint::Fill(1)],
        )
        .split(frame.area());

        self.url.set_block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .title("URL"),
        );
        frame.render_widget(&self.url, layout[0]);

        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title("Response");
        frame.render_widget(&block, layout[1]);
        frame.render_widget(
            Paragraph::new(self.response.as_str()).wrap(Wrap { trim: false }),
            block.inner(layout[1]),
        )
    }
}
