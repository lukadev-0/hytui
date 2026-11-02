use std::sync::Arc;

use anyhow::{Context, Result};
use async_executor::Executor;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use http::{HeaderName, HeaderValue, Method, Request, header};
use http_body_util::BodyExt;
use hyper::Uri;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Offset, Rect, Size, Spacing};
use ratatui::style::{Color, Style, Styled, Stylize};
use ratatui::symbols::merge::MergeStrategy;
use ratatui::text::{Line, Text};
use ratatui::widgets::{
    Block, BorderType, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};

use crate::text_field::{TextField, TextFieldState};

mod http_client;
mod text_field;

fn main() -> Result<()> {
    let ex = Arc::new(Executor::new());
    let (msg_sender, msg_recv) = async_channel::bounded(16);
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
                Ok(msg) => app.update(msg?),
                Err(_) => break,
            }

            while let Ok(msg) = msg_recv.try_recv() {
                app.update(msg?);
            }
        }

        anyhow::Ok(())
    }))?;

    ratatui::restore();

    Ok(())
}

struct App {
    ex: Arc<Executor<'static>>,
    msg_sender: async_channel::Sender<Result<Message>>,
    running: bool,
    focus: Focus,
    url: TextFieldState,
    body: TextFieldState,
    response: Text<'static>,
    response_width: usize,
    response_height: usize,
    response_scroll_v: ScrollbarState,
    response_scroll_h: ScrollbarState,
    method_idx: usize,
    headers: Vec<(HeaderName, HeaderValue)>,
    headers_focused_idx: usize,
    headers_editing: bool,
    headers_editing_field: TextFieldState,
    headers_scroll_v: ScrollbarState,
}

enum Message {
    Input(Event),
    Response(Text<'static>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Focus {
    Method,
    Url,
    Headers,
    Body,
    Response,
}

impl Focus {
    const ALL: &[Self] = &[
        Self::Method,
        Self::Url,
        Self::Headers,
        Self::Body,
        Self::Response,
    ];
}

const ALL_METHODS: &[Method] = &[
    Method::GET,
    Method::POST,
    Method::PUT,
    Method::PATCH,
    Method::DELETE,
    Method::QUERY,
    Method::HEAD,
    Method::OPTIONS,
];

const METHOD_COLORS: &[Color] = &[
    Color::Blue,
    Color::Green,
    Color::Yellow,
    Color::Indexed(214),
    Color::Red,
    Color::Cyan,
    Color::Indexed(147),
    Color::Magenta,
];

const DEFAULT_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

impl App {
    fn new(ex: Arc<Executor<'static>>, msg_sender: async_channel::Sender<Result<Message>>) -> Self {
        Self {
            ex,
            msg_sender,
            running: true,
            focus: Focus::Method,
            url: TextFieldState::new(),
            body: TextFieldState::new(),
            response: Text::default(),
            response_width: 0,
            response_height: 0,
            response_scroll_v: ScrollbarState::new(0),
            response_scroll_h: ScrollbarState::new(0),
            method_idx: 0,
            headers: vec![(header::USER_AGENT, DEFAULT_USER_AGENT.try_into().unwrap())],
            headers_focused_idx: 0,
            headers_editing: false,
            headers_editing_field: TextFieldState::new(),
            headers_scroll_v: ScrollbarState::new(0),
        }
    }

    fn update(&mut self, msg: Message) {
        match msg {
            Message::Input(input) => self.handle_input(input),
            Message::Response(res) => {
                self.response_width = res.width();
                self.response_height = res.height();
                self.response_scroll_v = ScrollbarState::new(0);
                self.response_scroll_h = ScrollbarState::new(0);
                self.response = res;
            }
        }
    }

    fn handle_input(&mut self, event: Event) {
        if let Event::Key(key_event) = event
            && event.is_key_press()
        {
            let has_ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
            match key_event.code {
                KeyCode::Char('c') if has_ctrl => {
                    self.running = false;
                }
                KeyCode::Tab => self.focus_next(),
                KeyCode::BackTab => self.focus_prev(),
                KeyCode::Enter | KeyCode::Char('j') if has_ctrl => {
                    self.handle_submit();
                }
                _ => match self.focus {
                    Focus::Method => self.handle_key_method(key_event),
                    Focus::Url => self.handle_key_url(key_event),
                    Focus::Headers => self.handle_key_headers(key_event),
                    Focus::Body => self.body.handle_input(key_event),
                    Focus::Response => self.handle_key_response(key_event),
                },
            }
        }
    }

    fn focus_prev(&mut self) {
        let mut focus_index = self.focus as usize;
        focus_index = focus_index.checked_sub(1).unwrap_or(Focus::ALL.len() - 1);
        self.focus = Focus::ALL[focus_index];
    }

    fn focus_next(&mut self) {
        let mut focus_index = self.focus as usize;
        focus_index = (focus_index + 1) % Focus::ALL.len();
        self.focus = Focus::ALL[focus_index];
    }

    fn handle_key_method(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Up => {
                self.method_idx = self
                    .method_idx
                    .checked_sub(1)
                    .unwrap_or(ALL_METHODS.len() - 1);
            }
            KeyCode::Down => {
                self.method_idx = (self.method_idx + 1) % ALL_METHODS.len();
            }
            KeyCode::Enter => self.focus_next(),
            _ => {}
        }
    }

    fn handle_key_url(&mut self, key_event: KeyEvent) {
        if key_event.code == KeyCode::Enter {
            self.focus_next();
        } else {
            self.url.handle_input(key_event);
        }
    }

    fn handle_key_headers(&mut self, key_event: KeyEvent) {
        if self.headers_editing {
            match key_event.code {
                KeyCode::Enter => {
                    let text = self.headers_editing_field.text.to_string();
                    let Some((header_name, header_value)) = text.split_once(':') else {
                        return;
                    };
                    let Ok(header_name) = header_name.trim().parse() else {
                        return;
                    };
                    let Ok(header_value) = header_value.trim().parse() else {
                        return;
                    };
                    if let Some(header) = self.headers.get_mut(self.headers_focused_idx) {
                        *header = (header_name, header_value);
                    } else {
                        self.headers.push((header_name, header_value));
                        self.headers_focused_idx += 1;
                    }
                    self.headers_editing = false;
                }
                KeyCode::Esc => {
                    self.headers_editing = false;
                }
                _ => self.headers_editing_field.handle_input(key_event),
            }
            return;
        }

        match key_event.code {
            KeyCode::Up => {
                self.headers_focused_idx = self
                    .headers_focused_idx
                    .checked_sub(1)
                    .unwrap_or(self.headers.len())
            }
            KeyCode::Down => {
                self.headers_focused_idx =
                    (self.headers_focused_idx + 1) % (self.headers.len() + 1);
            }
            KeyCode::Enter => {
                self.headers_editing = true;
                self.headers_editing_field.clear();
                if let Some((header_name, header_value)) =
                    self.headers.get(self.headers_focused_idx)
                {
                    let text = format!(
                        "{header_name}: {}",
                        String::from_utf8_lossy(header_value.as_bytes())
                    );
                    self.headers_editing_field.insert_str(&text);
                }
            }
            KeyCode::Delete | KeyCode::Char('d')
                if self.headers_focused_idx < self.headers.len() =>
            {
                self.headers.remove(self.headers_focused_idx);
            }
            _ => {}
        }
    }

    fn handle_key_response(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Up => self.response_scroll_v.prev(),
            KeyCode::Down => self.response_scroll_v.next(),
            KeyCode::Left => self.response_scroll_h.prev(),
            KeyCode::Right => self.response_scroll_h.next(),
            _ => {}
        }
    }

    fn handle_submit(&mut self) {
        let mut url = self.url.text.to_string();
        if url.is_empty() {
            return;
        }
        if !url.starts_with("http://") && !url.starts_with("https://") {
            url = format!("https://{url}");
        }

        let url: Uri = match url.parse() {
            Ok(url) => url,
            Err(err) => {
                self.response = err.to_string().red().into();
                return;
            }
        };

        self.url.clear();
        self.url.insert_str(&url.to_string());

        let mut req = match Request::builder()
            .uri(url)
            .method(&ALL_METHODS[self.method_idx])
            .body(
                self.body
                    .text
                    .to_string()
                    .map_err(anyhow::Error::new)
                    .boxed(),
            ) {
            Ok(req) => req,
            Err(err) => {
                self.response = err.to_string().red().into();
                return;
            }
        };

        req.headers_mut().extend(self.headers.clone());

        self.response = "Sending...".yellow().into();

        let msg_sender = self.msg_sender.clone();
        self.ex
            .spawn(async move {
                let res = http_client::request(req, async |res| {
                    let mut lines = Vec::<Line>::new();
                    let status_style = if res.status().is_success() {
                        Style::new().cyan().bold()
                    } else if res.status().is_client_error() || res.status().is_server_error() {
                        Style::new().magenta().bold()
                    } else {
                        Style::new().blue().bold()
                    };
                    lines.push(res.status().to_string().set_style(status_style).into());

                    for (header_name, header_value) in res.headers() {
                        lines.push(Line::from_iter([
                            format!("{header_name}: ").yellow(),
                            String::from_utf8_lossy(header_value.as_bytes())
                                .into_owned()
                                .green(),
                        ]));
                    }

                    let collected = res.collect().await.context("failed to collect response")?;
                    let bytes = collected.to_bytes();
                    let body = String::from_utf8_lossy(&bytes);

                    lines.push(Line::default());
                    lines.extend(body.lines().map(|l| l.to_string().into()));

                    Ok(lines)
                })
                .await
                .unwrap_or_else(|err| vec![format!("{err:?}").red().into()]);

                _ = msg_sender.send(Ok(Message::Response(res.into()))).await;
            })
            .detach();
    }

    fn focused_keys(&self) -> &'static [(&'static str, &'static str)] {
        match self.focus {
            Focus::Method => &[("↑↓", "select")],
            Focus::Url => &[("enter", "submit")],
            Focus::Headers => {
                if self.headers_editing {
                    &[("enter", "submit"), ("esc", "cancel")]
                } else {
                    &[("↑↓", "select"), ("enter", "edit")]
                }
            }
            Focus::Body => &[],
            Focus::Response => &[("←↑↓→", "scroll")],
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let layout =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).split(frame.area());
        self.render_main(frame, layout[0]);
        self.render_keys(frame, layout[1]);
    }

    fn border_style(&mut self, focus: Focus) -> Style {
        let mut style = Style::new();
        if self.focus == focus {
            style = style.blue();
        }
        style
    }

    fn render_main(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::horizontal([Constraint::Fill(1), Constraint::Fill(1)]).split(area);

        self.render_request(frame, layout[0]);
        self.render_response(frame, layout[1]);
    }

    fn render_keys(&mut self, frame: &mut Frame, area: Rect) {
        let keys = self.focused_keys();
        const GLOBAL_KEYS: &[(&str, &str)] = &[("ctrl-c", "quit"), ("tab", "cycle pane")];

        let mut x = 0u16;
        for &(key, desc) in keys {
            let text = Line::from_iter([key.bold(), " ".into(), desc.into()]);
            let rect = area
                .offset(Offset::new(x as _, 0))
                .resize(Size::new(text.width() as _, 1));
            frame.render_widget(&text, rect);
            x += rect.width + 2;
        }

        let mut x = area.right();
        for &(key, desc) in GLOBAL_KEYS {
            let text = Line::from_iter([key.bold(), " ".into(), desc.into()]);
            let width = text.width() as _;
            x -= width;
            let rect = area
                .offset(Offset::new(x as _, 0))
                .resize(Size::new(width, 1));
            frame.render_widget(&text, rect);
            x -= 2;
        }
    }

    fn render_request(&mut self, frame: &mut Frame, area: Rect) {
        let layout = Layout::vertical([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Fill(1),
        ])
        .split(area);

        self.render_request_url(frame, layout[0]);
        self.render_request_headers(frame, layout[1]);
        self.render_request_body(frame, layout[2]);
    }

    fn render_request_url(&mut self, frame: &mut Frame, area: Rect) {
        let method = &ALL_METHODS[self.method_idx];
        let method_color = METHOD_COLORS[self.method_idx];

        let layout = Layout::horizontal([Constraint::Length(11), Constraint::Fill(1)])
            .spacing(Spacing::Overlap(1))
            .split(area);

        frame.render_widget(
            Paragraph::new(Text::from(method.as_str()).style(method_color))
                .centered()
                .block(
                    Block::bordered()
                        .border_style(self.border_style(Focus::Method))
                        .border_type(BorderType::Rounded)
                        .merge_borders(MergeStrategy::Fuzzy),
                ),
            layout[0],
        );

        let url_block = Block::bordered()
            .border_style(self.border_style(Focus::Url))
            .border_type(BorderType::Rounded)
            .merge_borders(MergeStrategy::Fuzzy)
            .title("URL");
        let url_inner = url_block.inner(layout[1]);

        frame.render_widget(&url_block, layout[1]);
        frame.render_stateful_widget(TextField::new(), url_inner, &mut self.url);
        if self.focus == Focus::Url {
            frame.set_cursor_position(url_inner.as_position() + self.url.cursor_offset());
        }
    }

    fn render_request_headers(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(self.border_style(Focus::Headers))
            .border_type(BorderType::Rounded)
            .title("Headers");
        let inner = block.inner(area);
        frame.render_widget(&block, area);

        let scroll_y = self.headers_scroll_v.get_position();
        if self.headers_focused_idx < scroll_y {
            self.headers_scroll_v = self.headers_scroll_v.position(self.headers_focused_idx);
        }
        if self.headers_focused_idx >= scroll_y + inner.height as usize {
            self.headers_scroll_v = self
                .headers_scroll_v
                .position(self.headers_focused_idx - inner.height as usize + 1);
        }

        self.headers_scroll_v = self
            .headers_scroll_v
            .content_length(self.headers.len().saturating_sub(inner.height as usize - 1));
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area.inner(Margin::new(0, 1)),
            &mut self.headers_scroll_v,
        );

        let scroll_y = self.headers_scroll_v.get_position();

        let mut line_rect = Rect::new(inner.x, inner.y, inner.width, 1);
        for idx in scroll_y..self.headers.len().min(scroll_y + inner.height as usize) {
            let (header_name, header_value) = &self.headers[idx];
            let focused = self.headers_focused_idx == idx;
            if focused && self.headers_editing {
                self.render_request_headers_editing_field(frame, line_rect);
            } else {
                let mut line = Line::from_iter([
                    format!("{header_name}: ").yellow(),
                    String::from_utf8_lossy(header_value.as_bytes())
                        .into_owned()
                        .green(),
                ]);
                if self.focus == Focus::Headers && focused {
                    line = line.on_dark_gray();
                }
                frame.render_widget(&line, line_rect);
            }

            line_rect.y += 1;
        }

        if line_rect.y >= inner.bottom() {
            return;
        }

        let new_header_focused = self.headers_focused_idx == self.headers.len();
        if self.headers_editing && new_header_focused {
            self.render_request_headers_editing_field(frame, line_rect);
        } else {
            let mut line = Line::raw("+ add header").dim();
            if self.focus == Focus::Headers && new_header_focused {
                line = line.on_dark_gray().not_dim().blue();
            }
            frame.render_widget(&line, line_rect);
        }
    }

    fn render_request_headers_editing_field(&mut self, frame: &mut Frame, area: Rect) {
        frame.render_stateful_widget(TextField::new(), area, &mut self.headers_editing_field);
        if self.focus == Focus::Headers {
            frame.set_cursor_position(
                area.as_position() + self.headers_editing_field.cursor_offset(),
            );
        }
    }

    fn render_request_body(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(self.border_style(Focus::Body))
            .border_type(BorderType::Rounded)
            .title("Body");
        let inner = block.inner(area);
        frame.render_widget(&block, area);
        frame.render_stateful_widget(TextField::new(), inner, &mut self.body);
        if self.focus == Focus::Body {
            frame.set_cursor_position(inner.as_position() + self.body.cursor_offset());
        }
    }

    fn render_response(&mut self, frame: &mut Frame, area: Rect) {
        let block = Block::bordered()
            .border_style(self.border_style(Focus::Response))
            .border_type(BorderType::Rounded)
            .title("Response");
        let inner = block.inner(area);

        self.response_scroll_v = self.response_scroll_v.content_length(
            self.response_height
                .saturating_sub(inner.height as usize - 1),
        );
        self.response_scroll_h = self
            .response_scroll_h
            .content_length(self.response_width.saturating_sub(inner.width as usize - 1));

        frame.render_widget(
            Paragraph::new(self.response.clone())
                .scroll((
                    self.response_scroll_v.get_position() as _,
                    self.response_scroll_h.get_position() as _,
                ))
                .block(block),
            area,
        );

        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area.inner(Margin::new(0, 1)),
            &mut self.response_scroll_v,
        );
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::HorizontalBottom),
            area.inner(Margin::new(1, 0)),
            &mut self.response_scroll_h,
        );
    }
}
