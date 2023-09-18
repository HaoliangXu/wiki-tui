use ratatui::style::{Color, Modifier, Style};
use textwrap::wrap_algorithms::{wrap_optimal_fit, Penalties};
use tracing::warn;
use wiki_api::document::{Data, Document, Node};

use crate::renderer::Word;

use super::RenderedDocument;

const DISAMBIGUATION_PADDING: u8 = 1;
const DISAMBIGUATION_PREFIX: char = '|';

#[derive(Clone, Copy)]
enum Context {
    Normal,
    Header,
    WikiLink,
    MediaLink,
    ExternalLink,
    RedLink,
    Reflink,
}

struct Renderer {
    current_modifier: Style,
    rendered_lines: Vec<Vec<Word>>,
    current_line: Vec<Word>,
    contexts: Vec<Context>,
    width: u16,

    left_padding: u8,
    prefix: Option<char>,
}

impl<'a> Renderer {
    fn render_document(document: &'a Document, width: u16) -> RenderedDocument {
        if document.nodes.is_empty() {
            warn!("document contains no nodes, aborting the render");
            return RenderedDocument { lines: Vec::new() };
        }

        let mut renderer = Renderer {
            current_modifier: Style::default(),
            rendered_lines: Vec::new(),
            current_line: Vec::new(),
            contexts: Vec::new(),
            width,

            left_padding: 0,
            prefix: None,
        };

        renderer.render_node(document.nth(0).unwrap());

        RenderedDocument {
            lines: renderer.rendered_lines,
        }
    }

    /// Returns whether the last word of the current line is a whitespace
    fn is_last_whitespace(&self) -> bool {
        self.current_line
            .last()
            .map(|last| last.index == usize::MAX)
            .unwrap_or(false)
    }

    /// Returns whether the last rendered line is an empty one
    ///
    /// When the current line is not empty, this will return false
    fn is_last_empty(&self) -> bool {
        if !self.current_line.is_empty() {
            false
        } else {
            self.rendered_lines
                .last()
                .map(|last| last.is_empty())
                .unwrap_or(false)
        }
    }

    /// Adds a whitespace to the end of the current line
    ///
    /// The whitespace word has an index of `usize::MAX` and a width of `0` to not interfere with text wrapping. Note: If there already is a whitespace at the end of the current line, no whitespace will be added!
    fn add_whitespace(&mut self) {
        if self
            .current_line
            .last()
            .map(|word| word.index == usize::MAX)
            .unwrap_or(false)
        {
            return;
        }

        self.current_line.push(self.n_whitespace(1));
    }

    /// Returns a Word containing n amount of whitespace
    fn n_whitespace(&self, n: u8) -> Word {
        Word {
            index: usize::MAX,
            content: String::new(),
            style: Style::default(),
            width: 0.0,
            whitespace_width: n as f64,
            penalty_width: 0.0,
        }
    }

    /// Adds the specified Modifier
    fn add_modifier(&mut self, modifier: Modifier) {
        self.current_modifier = self.current_modifier.add_modifier(modifier);
    }

    /// Removes the specified Modifier
    fn remove_modifier(&mut self, modifier: Modifier) {
        self.current_modifier = self.current_modifier.remove_modifier(modifier);
    }

    /// Clears the current line
    ///
    /// When the current line is not empty already, it adds it to the rendered lines
    fn clear_line(&mut self) {
        if self.current_line.is_empty() {
            return;
        }

        self.rendered_lines
            .push(std::mem::take(&mut self.current_line));
    }

    /// Adds an empty line to the finished lines
    ///
    /// Clears the current line before adding the empty one
    fn add_empty_line(&mut self) {
        self.clear_line();
        self.rendered_lines.push(Vec::new());
    }

    /// Sets a new context
    ///
    /// Overrides the currently active context
    fn push_context(&mut self, context: Context) {
        self.contexts.push(context);
    }

    /// Returns the currently active context
    ///
    /// If no context is set, returns Context::Normal
    fn context(&self) -> Context {
        *self.contexts.last().unwrap_or(&Context::Normal)
    }

    /// Removes the currently active context
    ///
    /// The previously overriden context is set to the next active context
    fn pop_context(&mut self) {
        self.contexts.pop();
    }

    /// Returns the currently set style
    ///
    /// This combines the colors defined by the current context and the currently active modifiers
    fn current_style(&self) -> Style {
        let style = match self.context() {
            Context::Normal => Style::default(),
            Context::Header => Style::default().fg(Color::Red),
            Context::WikiLink => Style::default(),
            Context::MediaLink => Style::default(),
            Context::ExternalLink => Style::default(),
            Context::RedLink => Style::default().fg(Color::Red),
            Context::Reflink => Style::default().fg(Color::Gray),
        };

        style.patch(self.current_modifier)
    }

    /// Wraps and appends words
    ///
    /// This fills up the current line with words and wraps the remaining words into lines, appending them to the finished words. Note: This leaves the current line empty, except when there are not enough words to fill it up completely
    fn wrap_append(&mut self, words: Vec<Word>) {
        if words.is_empty() {
            return;
        }

        let mut current_width: f64 = 0.0;
        for word in self.current_line.iter() {
            current_width = current_width + word.width + word.whitespace_width;
        }

        let mut remaining_width = (self.width as f64) - current_width;

        // if the first word doesn't fit onto the current line, the line wrapping algorithm gets confuesed.
        // that means we have to clear it in this case
        if words.first().map(|word| word.width).unwrap_or_default() > remaining_width {
            remaining_width = self.width as f64;
            self.clear_line();
        }

        // when we start on a new line, we have to add the left padding and prefix to the line
        if self.current_line.is_empty() {
            remaining_width -= self.left_padding as f64;
            if let Some(prefix) = self.prefix {
                self.current_line.push(Word {
                    index: usize::MAX,
                    content: format!("{}{prefix}", " ".repeat(self.left_padding as usize)),
                    style: Style::default(),
                    width: 1.0,
                    whitespace_width: 1.0,
                    penalty_width: 0.0,
                });

                remaining_width -= 2.0; // subtract 2: 1 char and 1 whitespace
            }
        }

        let line_widths: [f64; 2] = [remaining_width, self.width as f64];
        let mut wrapped_lines: Vec<Vec<Word>> =
            wrap_optimal_fit(&words, &line_widths, &Penalties::default())
                .unwrap()
                .into_iter()
                .map(|word| word.to_vec())
                .collect();

        self.current_line.append(&mut wrapped_lines.remove(0));

        // add prefixes
        if let Some(prefix) = self.prefix {
            for line in wrapped_lines.iter_mut() {
                line.insert(
                    0,
                    Word {
                        index: usize::MAX,
                        content: prefix.to_string(),
                        style: Style::default(),
                        width: 1.0,
                        whitespace_width: 1.0,
                        penalty_width: 0.0,
                    },
                );
            }
        }

        // indent the current line
        for line in wrapped_lines.iter_mut() {
            line.insert(0, self.n_whitespace(self.left_padding));
        }

        if let Some(last_line) = wrapped_lines.pop() {
            self.clear_line();
            self.current_line = last_line;
            self.rendered_lines.append(&mut wrapped_lines)
        }
    }

    /// Adds an empty line only if the last line is not empty
    fn ensure_empty_line(&mut self) {
        if !self.is_last_empty() {
            self.add_empty_line();
        }
    }

    fn pre_children(&mut self, node: Node<'a>) {
        let mut is_block = false;
        match node.data() {
            Data::Section { id: _ } => is_block = true,
            Data::Header { id: _, kind: _ } => {
                self.push_context(Context::Header);
                self.add_modifier(Modifier::BOLD);
                is_block = true;
            }
            Data::Text { contents } => {
                const TEXT_SPECIAL_CHARACTERS: [char; 4] = [',', '.', '\"', '\''];
                if contents.starts_with(TEXT_SPECIAL_CHARACTERS) && self.is_last_whitespace() {
                    self.current_line.pop();
                }

                let has_trailing_whitespace = contents.ends_with(' ');
                let mut words: Vec<Word> = contents
                    .split_whitespace()
                    .map(|word| Word {
                        index: node.index(),
                        content: word.to_string(),
                        style: self.current_style(),
                        width: word.chars().count() as f64,
                        whitespace_width: 1.0,
                        penalty_width: 0.0,
                    })
                    .collect();

                if !has_trailing_whitespace {
                    if let Some(word) = words.last_mut() {
                        word.whitespace_width = 0.0;
                    }
                }

                self.wrap_append(words);
            }
            Data::Division => is_block = true,
            Data::Paragraph => is_block = true,
            Data::Span => {}
            Data::Reflink => {
                self.push_context(Context::Reflink);
                self.add_modifier(Modifier::ITALIC);
            }
            Data::Hatnote => is_block = true,
            Data::RedirectMessage => is_block = true,
            Data::Disambiguation => {
                is_block = true;
                self.add_modifier(Modifier::ITALIC);
                self.left_padding = DISAMBIGUATION_PADDING;
                self.prefix = Some(DISAMBIGUATION_PREFIX);
            }
            Data::OrderedList => is_block = true,
            Data::UnorderedList => is_block = true,
            Data::ListItem => self.clear_line(),
            Data::DescriptionList => is_block = true,
            Data::DescriptionListTerm => self.clear_line(),
            Data::DerscriptionListDescription => self.clear_line(),
            Data::Bold => self.add_modifier(Modifier::BOLD),
            Data::Italic => self.add_modifier(Modifier::ITALIC),
            Data::WikiLink { href: _, title: _ } => {
                self.push_context(Context::WikiLink);
                self.add_modifier(Modifier::UNDERLINED);
            }
            Data::RedLink { title: _ } => {
                self.push_context(Context::RedLink);
                self.add_modifier(Modifier::ITALIC);
                self.add_modifier(Modifier::UNDERLINED);
            }
            Data::MediaLink { href: _, title: _ } => {
                self.push_context(Context::MediaLink);
                self.add_modifier(Modifier::ITALIC);
                self.add_modifier(Modifier::UNDERLINED);
            }
            Data::ExternalLink {
                href: _,
                title: _,
                autonumber: _,
            } => {
                self.push_context(Context::ExternalLink);
                self.add_modifier(Modifier::ITALIC);
                self.add_modifier(Modifier::UNDERLINED);
            }
            Data::Unknown => {}
        }

        if is_block {
            self.ensure_empty_line();
        }
    }

    fn post_children(&mut self, node: Node<'a>) {
        let mut is_block = false;
        match node.data() {
            Data::Section { id: _ } => is_block = true,
            Data::Header { id: _, kind: _ } => {
                self.remove_modifier(Modifier::BOLD);
                self.pop_context();
                is_block = true;
            }
            Data::Text { contents: _ } => {}
            Data::Division => is_block = true,
            Data::Paragraph => is_block = true,
            Data::Span => self.add_whitespace(),
            Data::Reflink => {
                self.add_whitespace();
                self.pop_context();
                self.remove_modifier(Modifier::ITALIC);
            }
            Data::Hatnote => is_block = true,
            Data::RedirectMessage => is_block = true,
            Data::Disambiguation => {
                is_block = true;
                self.remove_modifier(Modifier::ITALIC);
                self.left_padding = self.left_padding.saturating_sub(DISAMBIGUATION_PADDING);
                self.prefix = None;
            }
            Data::OrderedList => is_block = true,
            Data::UnorderedList => is_block = true,
            Data::ListItem => self.clear_line(),
            Data::DescriptionList => is_block = true,
            Data::DescriptionListTerm => self.clear_line(),
            Data::DerscriptionListDescription => self.clear_line(),
            Data::Bold => self.remove_modifier(Modifier::BOLD),
            Data::Italic => self.remove_modifier(Modifier::ITALIC),
            Data::WikiLink { href: _, title: _ } => {
                self.pop_context();
                self.remove_modifier(Modifier::UNDERLINED);
                self.add_whitespace();
            }
            Data::RedLink { title: _ } => {
                self.pop_context();
                self.remove_modifier(Modifier::ITALIC);
                self.remove_modifier(Modifier::UNDERLINED);
                self.add_whitespace();
            }
            Data::MediaLink { href: _, title: _ } => {
                self.pop_context();
                self.remove_modifier(Modifier::ITALIC);
                self.remove_modifier(Modifier::UNDERLINED);
                self.add_whitespace();
            }
            Data::ExternalLink {
                href: _,
                title: _,
                autonumber: _,
            } => {
                self.pop_context();
                self.remove_modifier(Modifier::ITALIC);
                self.remove_modifier(Modifier::UNDERLINED);
                self.add_whitespace();
            }
            Data::Unknown => {}
        }

        if is_block {
            self.ensure_empty_line();
        }
    }

    fn render_node(&mut self, node: Node<'a>) {
        self.pre_children(node);
        for child in node.children() {
            self.render_node(child);
        }
        self.post_children(node);
    }
}

pub fn render_document(document: &Document, width: u16) -> RenderedDocument {
    Renderer::render_document(document, width)
}
