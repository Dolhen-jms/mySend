//! Champ de saisie sur une ligne.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::Frame;
use unicode_width::UnicodeWidthChar;

#[derive(Default, Clone)]
pub struct Input {
    pub chars: Vec<char>,
    pub cursor: usize,
    /// Champ de mot de passe : affiché en `•`.
    pub mask: bool,
}

impl Input {
    pub fn new(text: &str, cursor: Option<usize>) -> Input {
        let chars: Vec<char> = text.chars().collect();
        let c = cursor.unwrap_or(chars.len()).min(chars.len());
        Input { chars, cursor: c, mask: false }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len();
    }

    /// Retourne vrai si le texte a changé.
    pub fn handle(&mut self, k: &KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char(c) if ctrl => match c {
                'a' => self.cursor = 0,
                'e' => self.cursor = self.chars.len(),
                'b' => self.cursor = self.cursor.saturating_sub(1),
                'f' => self.cursor = (self.cursor + 1).min(self.chars.len()),
                'u' => {
                    self.chars.drain(..self.cursor);
                    self.cursor = 0;
                    return true;
                }
                'k' => {
                    self.chars.truncate(self.cursor);
                    return true;
                }
                'w' | 'h' => {
                    if c == 'h' {
                        return self.backspace();
                    }
                    return self.delete_word();
                }
                _ => {}
            },
            KeyCode::Backspace if ctrl || alt => return self.delete_word(),
            KeyCode::Char(c) if !alt => {
                self.chars.insert(self.cursor, c);
                self.cursor += 1;
                return true;
            }
            KeyCode::Backspace => return self.backspace(),
            KeyCode::Delete => {
                if self.cursor < self.chars.len() {
                    self.chars.remove(self.cursor);
                    return true;
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.chars.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.chars.len(),
            _ => {}
        }
        false
    }

    fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.chars.remove(self.cursor);
            return true;
        }
        false
    }

    fn delete_word(&mut self) -> bool {
        let end = self.cursor;
        let mut i = end;
        while i > 0 && self.chars[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && !matches!(self.chars[i - 1], ' ' | '/') {
            i -= 1;
        }
        if i == end && i > 0 {
            i -= 1;
        }
        self.chars.drain(i..end);
        self.cursor = i;
        end != i
    }

    /// Dessine le texte (défilement horizontal) et place le curseur.
    pub fn render(&self, f: &mut Frame, area: Rect, style: Style, placeholder: &str, focused: bool) {
        if area.width == 0 {
            return;
        }
        if self.chars.is_empty() && !placeholder.is_empty() {
            f.render_widget(
                Line::from(Span::styled(placeholder.to_string(), crate::style::dim())),
                area,
            );
        }
        let w = area.width as usize;
        let shown: Vec<char> = if self.mask { vec!['•'; self.chars.len()] } else { self.chars.clone() };
        // fenêtre visible contenant le curseur
        let widths: Vec<usize> = shown.iter().map(|c| c.width().unwrap_or(0)).collect();
        let mut start = 0;
        let cur_w = |s: usize| widths[s..self.cursor].iter().sum::<usize>();
        while start < self.cursor && cur_w(start) >= w {
            start += 1;
        }
        let mut s = String::new();
        let mut used = 0;
        for (i, c) in shown.iter().enumerate().skip(start) {
            if used + widths[i] > w {
                break;
            }
            used += widths[i];
            s.push(*c);
        }
        if !self.chars.is_empty() {
            f.render_widget(Line::from(Span::styled(s, style)), area);
        }
        if focused {
            let x = area.x + cur_w(start).min(w.saturating_sub(1)) as u16;
            f.set_cursor_position((x, area.y));
        }
    }
}
