use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    prelude::{Margin, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};
use tracing::{debug, info, warn};
use wiki_api::{
    document::{Data, Node},
    page::{Link, Page, Section},
};

use crate::{
    action::{Action, ActionResult, PageAction},
    components::Component,
    has_modifier, key_event,
    renderer::{default_renderer::render_document, RenderedDocument},
    terminal::Frame,
    ui::padded_rect,
};

#[cfg(debug_assertions)]
use crate::renderer::test_renderer::{render_nodes_raw, render_tree_data, render_tree_raw};

const SCROLLBAR: bool = true;
const LINK_SELECT: bool = true;

#[derive(Default, Debug, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum Renderer {
    #[default]
    Default,

    #[cfg(debug_assertions)]
    TestRendererTreeData,
    #[cfg(debug_assertions)]
    TestRendererTreeRaw,
    #[cfg(debug_assertions)]
    TestRendererNodeRaw,
}

impl Renderer {
    pub fn next(&self) -> Self {
        match self {
            #[cfg(not(debug_assertions))]
            &Renderer::Default => Renderer::Default,

            #[cfg(debug_assertions)]
            &Renderer::Default => Renderer::TestRendererTreeData,
            #[cfg(debug_assertions)]
            &Renderer::TestRendererTreeData => Renderer::TestRendererTreeRaw,
            #[cfg(debug_assertions)]
            &Renderer::TestRendererTreeRaw => Renderer::TestRendererNodeRaw,
            #[cfg(debug_assertions)]
            &Renderer::TestRendererNodeRaw => Renderer::Default,
        }
    }
}

#[derive(Default)]
struct PageContentsState {
    list_state: ListState,
    max_idx_section: u8,
}

pub struct PageComponent {
    page: Page,
    renderer: Renderer,
    render_cache: HashMap<u16, RenderedDocument>,
    viewport: Rect,
    selected: (usize, usize),

    is_contents: bool,
    contents_state: PageContentsState,
}

impl PageComponent {
    pub fn new(page: Page) -> Self {
        let contents_state = PageContentsState {
            list_state: ListState::default().with_selected(Some(0)),
            max_idx_section: page.sections().map(|x| x.len() as u8).unwrap_or_default(),
        };
        Self {
            page,
            renderer: Renderer::default(),
            render_cache: HashMap::new(),
            viewport: Rect::default(),
            selected: (0, 0),

            is_contents: false,
            contents_state,
        }
    }

    fn render_page(&self, width: u16) -> RenderedDocument {
        match self.renderer {
            Renderer::Default => render_document(&self.page.content, width),
            #[cfg(debug_assertions)]
            Renderer::TestRendererTreeData => render_tree_data(&self.page.content),
            #[cfg(debug_assertions)]
            Renderer::TestRendererTreeRaw => render_tree_raw(&self.page.content),
            #[cfg(debug_assertions)]
            Renderer::TestRendererNodeRaw => render_nodes_raw(&self.page.content),
        }
    }

    fn render_contents(&mut self, f: &mut Frame<'_>, area: Rect) {
        let sections = self.page.sections.as_ref();
        let block = Block::default()
            .title("Contents")
            .borders(Borders::ALL)
            .border_style({
                if self.is_contents {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                }
            });

        if sections.is_none() {
            f.render_widget(Paragraph::new("No Contents available").block(block), area);
            return;
        }

        let sections = sections.unwrap();
        let list = List::new(sections.iter().map(|x| format!("{} {}", x.number, x.text)))
            .block(block)
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            );
        f.render_stateful_widget(list, area, &mut self.contents_state.list_state);
    }

    fn selected_header(&self) -> Option<&Section> {
        let sections = self.page.sections()?;
        let section_idx = self.contents_state.list_state.selected()?;
        assert!(section_idx < self.contents_state.max_idx_section as usize);

        Some(&sections[section_idx])
    }

    fn switch_renderer(&mut self, renderer: Renderer) {
        self.renderer = renderer;
        self.flush_cache();
    }

    fn flush_cache(&mut self) {
        debug!("flushing '{}' cached renders", self.render_cache.len());
        self.render_cache.clear();
        if LINK_SELECT {
            self.selected = (0, 0);
        }
    }

    fn scroll_down(&mut self, amount: u16) {
        if self.is_contents {
            let i = match self.contents_state.list_state.selected() {
                Some(i) => {
                    if i >= self.contents_state.max_idx_section as usize - 1 {
                        0
                    } else {
                        i + 1
                    }
                }
                None => 0,
            };

            self.contents_state.list_state.select(Some(i));
            return;
        }

        if let Some(page) = self.render_cache.get(&self.viewport.width) {
            let n_lines = page.lines.len() as u16;
            if self.viewport.bottom() + amount >= n_lines {
                self.viewport.y = n_lines.saturating_sub(self.viewport.height);
                return;
            }
        }
        self.viewport.y += amount;
    }

    fn scroll_up(&mut self, amount: u16) {
        if self.is_contents {
            let i = match self.contents_state.list_state.selected() {
                Some(i) => {
                    if i == 0 {
                        self.contents_state.max_idx_section as usize - 1
                    } else {
                        i - 1
                    }
                }
                None => 0,
            };

            self.contents_state.list_state.select(Some(i));
            return;
        }

        self.viewport.y = self.viewport.y.saturating_sub(amount);
    }

    fn select_first(&mut self) {
        if self.page.content.nth(0).is_none() {
            return;
        }

        let selectable_node = self
            .page
            .content
            .nth(0)
            .unwrap()
            .descendants()
            .find(|node| matches!(node.data(), &Data::Link(_)));

        if let Some(selectable_node) = selectable_node {
            let first_index = selectable_node.index();
            let last_index = selectable_node
                .last_child()
                .map(|child| child.index())
                .unwrap_or(first_index);
            self.selected = (first_index, last_index);
        }
    }

    fn select_prev(&mut self) {
        if self.page.content.nth(0).is_none() {
            return;
        }

        let selectable_node = self
            .page
            .content
            .nth(0)
            .unwrap()
            .descendants()
            .filter(|node| matches!(node.data(), &Data::Link(_)) && node.index() < self.selected.0)
            .last();

        if let Some(selectable_node) = selectable_node {
            let first_index = selectable_node.index();
            let last_index = selectable_node
                .last_child()
                .map(|child| child.index())
                .unwrap_or(first_index);
            self.selected = (first_index, last_index);
        }
    }

    fn select_next(&mut self) {
        if self.page.content.nth(0).is_none() {
            return;
        }

        let selectable_node = self
            .page
            .content
            .nth(0)
            .unwrap()
            .descendants()
            .find(|node| matches!(node.data(), &Data::Link(_)) && self.selected.1 < node.index());

        if let Some(selectable_node) = selectable_node {
            let first_index = selectable_node.index();
            let last_index = selectable_node
                .last_child()
                .map(|child| child.index())
                .unwrap_or(first_index);
            self.selected = (first_index, last_index);
        }
    }

    fn select_last(&mut self) {
        if self.page.content.nth(0).is_none() {
            return;
        }

        let selectable_node = self
            .page
            .content
            .nth(0)
            .unwrap()
            .descendants()
            .filter(|node| matches!(node.data(), &Data::Link(_)) && node.index() > self.selected.1)
            .last();

        if let Some(selectable_node) = selectable_node {
            let first_index = selectable_node.index();
            let last_index = selectable_node
                .last_child()
                .map(|child| child.index())
                .unwrap_or(first_index);
            self.selected = (first_index, last_index);
        }
    }

    fn open_link(&self) -> ActionResult {
        let index = self.selected.0;
        let node = Node::new(&self.page.content, index).unwrap();
        let data = node.data().to_owned();

        match data {
            Data::Link(Link::Internal(link_data)) => Action::LoadPage(link_data.page).into(),
            _ => ActionResult::consumed(),
        }
    }

    fn resize(&mut self, width: u16, height: u16) {
        self.viewport.width = width;
        self.viewport.height = height;

        self.flush_cache();
    }

    fn select_header(&mut self, anchor: String) {
        // HACK: do not hardcode this
        if &anchor == "Content_Top" {
            info!("special case: jumping to top");
            self.viewport.y = 0;
            return;
        }

        let header_node = self
            .page
            .content
            .nth(0)
            .unwrap()
            .descendants()
            .filter(|node| {
                if let Data::Header { id, .. } = node.data() {
                    id == &anchor
                } else {
                    false
                }
            })
            .last();

        if header_node.is_none() {
            warn!("no header with the anchor '{}' could be found", anchor);
            return;
        }

        let header_node = header_node.unwrap();
        let first_index = header_node.index();
        let last_index = header_node
            .last_child()
            .map(|child| child.index())
            .unwrap_or(first_index);

        for (y, line) in self
            .render_page(self.viewport.width)
            .lines
            .iter()
            .enumerate()
        {
            for word in line {
                if let Some(node) = word.node(&self.page.content) {
                    if node.index() <= last_index && node.index() >= first_index {
                        self.viewport.y = y as u16;
                        return;
                    }
                }
            }
        }

        warn!("no word could be matched to the header node");
    }
}

impl Component for PageComponent {
    fn handle_key_events(&mut self, key: KeyEvent) -> ActionResult {
        if self.is_contents {
            return match key.code {
                KeyCode::Char('t') => Action::Page(PageAction::ToggleContents).into(),
                KeyCode::Enter if self.contents_state.list_state.selected().is_some() => {
                    let header = self.selected_header();
                    if header.is_none() {
                        info!("no header selected");
                        return ActionResult::Ignored;
                    }
                    Action::Page(PageAction::GoToHeader(header.unwrap().anchor.to_string())).into()
                }
                _ => ActionResult::Ignored,
            };
        }

        match key.code {
            KeyCode::Char('r') if has_modifier!(key, Modifier::CONTROL) => {
                Action::Page(PageAction::SwitchRenderer(self.renderer.next())).into()
            }
            KeyCode::Char('t') => Action::Page(PageAction::ToggleContents).into(),
            KeyCode::Left if has_modifier!(key, Modifier::SHIFT) => {
                Action::Page(PageAction::SelectFirstLink).into()
            }
            KeyCode::Right if has_modifier!(key, Modifier::SHIFT) => {
                Action::Page(PageAction::SelectLastLink).into()
            }
            KeyCode::Up if has_modifier!(key, Modifier::SHIFT) => {
                Action::Page(PageAction::SelectTopLink).into()
            }
            KeyCode::Down if has_modifier!(key, Modifier::SHIFT) => {
                Action::Page(PageAction::SelectBottomLink).into()
            }
            KeyCode::Left => Action::Page(PageAction::SelectPrevLink).into(),
            KeyCode::Right => Action::Page(PageAction::SelectNextLink).into(),
            KeyCode::Enter => self.open_link(),
            _ => ActionResult::Ignored,
        }
    }

    fn keymap(&self) -> super::help::Keymap {
        vec![
            (
                key_event!('r', Modifier::CONTROL),
                Action::Page(PageAction::SwitchRenderer(self.renderer.next())).into(),
            ),
            (
                key_event!(Key::Left, Modifier::SHIFT),
                Action::Page(PageAction::SelectFirstLink).into(),
            ),
            (
                key_event!(Key::Left),
                Action::Page(PageAction::SelectPrevLink).into(),
            ),
            (
                key_event!(Key::Right, Modifier::SHIFT),
                Action::Page(PageAction::SelectLastLink).into(),
            ),
            (
                key_event!(Key::Right),
                Action::Page(PageAction::SelectNextLink).into(),
            ),
            (
                key_event!(Key::Up, Modifier::SHIFT),
                Action::Page(PageAction::SelectTopLink).into(),
            ),
            (
                key_event!(Key::Down, Modifier::SHIFT),
                Action::Page(PageAction::SelectBottomLink).into(),
            ),
        ]
    }

    fn update(&mut self, action: Action) -> ActionResult {
        match action {
            Action::Page(page_action) => match page_action {
                PageAction::SwitchRenderer(renderer) => self.switch_renderer(renderer),
                PageAction::ToggleContents => self.is_contents = !self.is_contents,

                PageAction::SelectFirstLink => self.select_first(),
                PageAction::SelectLastLink => self.select_last(),

                PageAction::SelectTopLink | PageAction::SelectBottomLink => todo!(),

                PageAction::SelectPrevLink => self.select_prev(),
                PageAction::SelectNextLink => self.select_next(),

                PageAction::GoToHeader(anchor) => self.select_header(anchor),
            },
            Action::ScrollUp(amount) => self.scroll_up(amount),
            Action::ScrollDown(amount) => self.scroll_down(amount),

            Action::ScrollHalfUp => self.scroll_up(self.viewport.height / 2),
            Action::ScrollHalfDown => self.scroll_down(self.viewport.height / 2),

            Action::ScrollToTop => self.viewport.y = 0,
            Action::ScrollToBottom => {
                if let Some(page) = self.render_cache.get(&self.viewport.width) {
                    self.scroll_down(page.lines.len() as u16)
                }
            }

            Action::Resize(width, heigth) => self.resize(width, heigth),
            _ => return ActionResult::Ignored,
        }
        ActionResult::consumed()
    }

    fn render(&mut self, f: &mut Frame, area: Rect) {
        let (area, status_area) = {
            let splits = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(100), Constraint::Min(1)])
                .split(padded_rect(area, 1, 1));
            (splits[0], splits[1])
        };

        let status_msg = format!(
            " wiki-tui | Page '{}' | Language '{}' | '{}' other languages available",
            self.page.title,
            self.page.language.name(),
            self.page.available_languages().unwrap_or_default()
        );
        f.render_widget(Paragraph::new(status_msg), status_area);

        let area = {
            let splits = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
                .split(area);

            self.render_contents(f, splits[1]);
            splits[0]
        };

        let page_area = if SCROLLBAR {
            area.inner(&Margin {
                vertical: 0,
                horizontal: 2, // for the scrollbar
            })
        } else {
            area
        };

        self.viewport.width = page_area.width;
        self.viewport.height = page_area.height;

        let rendered_page = match self.render_cache.get(&page_area.width) {
            Some(rendered_page) => rendered_page,
            None => {
                let rendered_page = self.render_page(page_area.width);
                info!("rebuilding cache for '{}'", page_area.width);
                self.render_cache.insert(page_area.width, rendered_page);
                self.render_cache.get(&page_area.width).unwrap()
            }
        };

        let mut lines: Vec<Line> = rendered_page
            .lines
            .iter()
            .skip(self.viewport.top() as usize)
            .take(self.viewport.bottom() as usize)
            .map(|line| {
                let mut spans: Vec<Span> = Vec::new();
                line.iter()
                    .map(|word| {
                        let mut span = Span::styled(
                            format!(
                                "{}{}",
                                word.content,
                                " ".repeat(word.whitespace_width as usize)
                            ),
                            word.style,
                        );

                        if let Some(node) = word.node(&self.page.content) {
                            let index = node.index();
                            if self.selected.0 <= index && index <= self.selected.1 {
                                span.patch_style(Style::new().add_modifier(Modifier::UNDERLINED))
                            }
                        }

                        spans.push(span);
                    })
                    .count();
                Line {
                    spans,
                    ..Default::default()
                }
            })
            .collect();

        if self.viewport.y == 0 {
            let mut title_line = Line::raw(&self.page.title);
            title_line.patch_style(Style::default().fg(Color::Red).bold());

            lines.insert(0, title_line);
            lines.pop();
        }

        if SCROLLBAR {
            let scrollbar = Scrollbar::default()
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some(" "))
                .track_style(Style::new().black().on_black())
                .thumb_style(Style::new().blue())
                .orientation(ScrollbarOrientation::VerticalRight);
            let mut scrollbar_state = ScrollbarState::new(
                rendered_page
                    .lines
                    .len()
                    .saturating_sub(self.viewport.height as usize),
            )
            .position(self.viewport.top() as usize);
            f.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
        }

        f.render_widget(Paragraph::new(lines), page_area);
    }
}
