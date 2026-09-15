use std::collections::{HashMap, HashSet};
use std::ops::Range;

use ratatui::prelude::{Color, Line, Modifier, Span, Style};
use tui_markdown::{
    AlertKind, Options, RenderContext, StreamingMarkdown, StyleSheet, WorkCounters,
};

use super::ChatMessage;

#[derive(Clone, Copy, Debug, Default)]
struct AgentStyleSheet;

impl StyleSheet for AgentStyleSheet {
    fn heading(&self, _level: u8) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    fn code(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::REVERSED)
    }

    fn link(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::UNDERLINED)
    }

    fn blockquote(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::DIM | Modifier::ITALIC)
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
    }

    fn heading_marker(&self, _level: u8) -> &str {
        ""
    }

    fn code_block_fence(&self) -> &str {
        ""
    }

    fn html(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
    }

    fn math_inline(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::ITALIC)
    }

    fn math_display(&self) -> Style {
        Style::new().fg(Color::Reset)
    }

    fn footnote_ref(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::DIM | Modifier::ITALIC)
    }

    fn footnote_def(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
    }

    fn definition_term(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD)
    }

    fn definition_description(&self) -> Style {
        Style::new().fg(Color::Reset)
    }

    fn alert(&self, _kind: AlertKind) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::BOLD)
    }

    fn table_header(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    fn table_cell(&self) -> Style {
        Style::new().fg(Color::Reset)
    }

    fn table_border(&self) -> Style {
        Style::new().fg(Color::Reset).add_modifier(Modifier::DIM)
    }

    fn image_alt(&self) -> Style {
        Style::new()
            .fg(Color::Reset)
            .add_modifier(Modifier::DIM | Modifier::ITALIC)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AgentMarkdownKey {
    Active(usize),
    History {
        turn_index: usize,
        detail_index: usize,
    },
}

struct AgentMarkdownProjection {
    identity: u64,
    context: RenderContext,
    document: StreamingMarkdown<AgentStyleSheet>,
    finished: bool,
    first_content_row: Option<usize>,
    prepared: Option<PreparedRows>,
}

struct PreparedRows {
    range: Range<usize>,
    lines: Vec<Line<'static>>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AgentMarkdownDiagnostics {
    pub processed_source_bytes: u64,
    pub parsed_events: u64,
    pub rendered_events: u64,
    pub recomputed_blocks: u64,
    pub full_recomputations: u64,
    pub context_reflows: u64,
    pub materialized_rows: u64,
    pub projection_updates: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct AgentMarkdownResourceUsage {
    pub retained_documents: usize,
    pub source_bytes: usize,
    pub source_capacity_bytes: usize,
    pub current_capacity_bytes: usize,
    pub semantic_capacity_bytes: usize,
}

impl AgentMarkdownDiagnostics {
    fn record_work(&mut self, work: WorkCounters) {
        self.processed_source_bytes = self
            .processed_source_bytes
            .saturating_add(work.processed_source_bytes);
        self.parsed_events = self.parsed_events.saturating_add(work.parsed_events);
        self.rendered_events = self.rendered_events.saturating_add(work.rendered_events);
        self.recomputed_blocks = self
            .recomputed_blocks
            .saturating_add(work.recomputed_blocks);
        self.full_recomputations = self
            .full_recomputations
            .saturating_add(work.full_recomputations);
        self.context_reflows = self.context_reflows.saturating_add(work.context_reflows);
    }
}

#[derive(Default)]
pub(crate) struct AgentMarkdownState {
    active: HashMap<usize, AgentMarkdownProjection>,
    history: HashMap<(usize, usize), AgentMarkdownProjection>,
    used_history: HashSet<(usize, usize)>,
    next_identity: u64,
    diagnostics: AgentMarkdownDiagnostics,
}

impl AgentMarkdownState {
    pub(crate) fn clear(&mut self) {
        self.active.clear();
        self.history.clear();
        self.used_history.clear();
        self.next_identity = self.next_identity.wrapping_add(1);
    }

    pub(crate) fn clear_active(&mut self) {
        self.active.clear();
        self.next_identity = self.next_identity.wrapping_add(1);
    }

    pub(crate) fn clear_history(&mut self) {
        self.history.clear();
        self.used_history.clear();
        self.next_identity = self.next_identity.wrapping_add(1);
    }

    pub(crate) fn move_active_to_history(&mut self, turn_index: usize, messages: &[ChatMessage]) {
        let mut detail_index = 0;
        for (message_index, message) in messages.iter().enumerate() {
            if matches!(message, ChatMessage::User(_)) {
                continue;
            }
            if let ChatMessage::Agent(source) = message {
                if let Some(body_width) = self
                    .active
                    .get(&message_index)
                    .map(|projection| projection.context.width())
                {
                    self.prepare_row_count(
                        AgentMarkdownKey::Active(message_index),
                        source,
                        body_width,
                        true,
                    );
                    if let Some(projection) = self.active.remove(&message_index) {
                        self.history.insert((turn_index, detail_index), projection);
                    }
                }
            }
            detail_index += 1;
        }
        self.active.clear();
    }

    pub(crate) fn begin_history_viewport(&mut self) {
        self.used_history.clear();
    }

    pub(crate) fn finish_history_viewport(&mut self) {
        self.history
            .retain(|key, _| self.used_history.contains(key));
    }

    pub(crate) fn retain_active_messages(&mut self, messages: &[ChatMessage]) {
        self.active.retain(|index, projection| {
            matches!(
                messages.get(*index),
                Some(ChatMessage::Agent(source))
                    if source.starts_with(projection.document.source())
            )
        });
    }

    pub(crate) fn history_sources_match(&self, turn_index: usize, details: &[ChatMessage]) -> bool {
        details.iter().enumerate().all(|(detail_index, message)| {
            let ChatMessage::Agent(source) = message else {
                return true;
            };
            self.history
                .get(&(turn_index, detail_index))
                .is_none_or(|projection| projection.document.source() == source)
        })
    }

    pub(crate) fn prepare(
        &mut self,
        key: AgentMarkdownKey,
        source: &str,
        body_width: u16,
        finished: bool,
    ) -> Vec<Line<'static>> {
        let row_count = self.prepare_row_count(key, source, body_width, finished);
        self.materialize_rows(key, 0..row_count)
    }

    pub(crate) fn prepare_row_count(
        &mut self,
        key: AgentMarkdownKey,
        source: &str,
        body_width: u16,
        finished: bool,
    ) -> usize {
        let context = RenderContext::new(body_width);
        let is_new = match key {
            AgentMarkdownKey::Active(index) => !self.active.contains_key(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => !self.history.contains_key(&(turn_index, detail_index)),
        };
        if is_new {
            let identity = self.next_identity;
            self.next_identity = self.next_identity.wrapping_add(1);
            let projection = AgentMarkdownProjection {
                identity,
                context,
                document: StreamingMarkdown::new(Options::new(AgentStyleSheet), context),
                finished: false,
                first_content_row: None,
                prepared: None,
            };
            match key {
                AgentMarkdownKey::Active(index) => {
                    self.active.insert(index, projection);
                }
                AgentMarkdownKey::History {
                    turn_index,
                    detail_index,
                } => {
                    self.history.insert((turn_index, detail_index), projection);
                }
            }
        }

        if let AgentMarkdownKey::History {
            turn_index,
            detail_index,
        } = key
        {
            self.used_history.insert((turn_index, detail_index));
        }

        let next_identity = &mut self.next_identity;
        let diagnostics = &mut self.diagnostics;
        let projection = match key {
            AgentMarkdownKey::Active(index) => self.active.get_mut(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => self.history.get_mut(&(turn_index, detail_index)),
        }
        .expect("Markdown projection was inserted");

        let before = projection.document.counters();
        let mut changed = false;
        if projection.context != context {
            projection.document.set_context(context);
            projection.context = context;
            changed = true;
        }

        let current_source = projection.document.source();
        if current_source != source {
            let can_append = !projection.finished && source.starts_with(current_source);
            if can_append {
                projection.document.append(&source[current_source.len()..]);
            } else {
                projection.identity = *next_identity;
                *next_identity = next_identity.wrapping_add(1);
                projection.document.replace(source);
            }
            projection.finished = false;
            changed = true;
        }
        if finished && !projection.finished {
            projection.document.finish();
            projection.finished = true;
            changed = true;
        }

        diagnostics.record_work(projection.document.counters() - before);
        if changed {
            projection.first_content_row = projection
                .document
                .current()
                .lines
                .iter()
                .position(|line| line.spans.iter().any(|span| !span.content.is_empty()));
            projection.prepared = None;
            diagnostics.projection_updates = diagnostics.projection_updates.saturating_add(1);
        }
        projection.document.current().lines.len()
    }

    pub(crate) fn materialize_rows(
        &mut self,
        key: AgentMarkdownKey,
        requested: Range<usize>,
    ) -> Vec<Line<'static>> {
        let diagnostics = &mut self.diagnostics;
        let projection = match key {
            AgentMarkdownKey::Active(index) => self.active.get_mut(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => self.history.get_mut(&(turn_index, detail_index)),
        }
        .expect("Markdown projection must be prepared before materializing rows");
        let prepared = projection.document.prepare_rows(
            requested.start,
            requested.end.saturating_sub(requested.start),
        );
        let range =
            prepared.first_row()..prepared.first_row().saturating_add(prepared.rows().len());

        if let Some(prepared) = &projection.prepared {
            if range.start >= prepared.range.start && range.end <= prepared.range.end {
                let start = range.start - prepared.range.start;
                let end = range.end - prepared.range.start;
                return prepared.lines[start..end].to_vec();
            }
        }

        let lines = prefixed_lines(prepared.rows(), range.start, projection.first_content_row);
        diagnostics.materialized_rows = diagnostics
            .materialized_rows
            .saturating_add(lines.len() as u64);
        projection.prepared = Some(PreparedRows {
            range,
            lines: lines.clone(),
        });
        lines
    }

    pub(crate) fn diagnostics(&self) -> AgentMarkdownDiagnostics {
        self.diagnostics
    }

    pub(crate) fn resource_usage(&self) -> AgentMarkdownResourceUsage {
        self.active.values().chain(self.history.values()).fold(
            AgentMarkdownResourceUsage::default(),
            |mut total, projection| {
                let usage = projection.document.resource_usage();
                total.retained_documents = total.retained_documents.saturating_add(1);
                total.source_bytes = total.source_bytes.saturating_add(usage.source_bytes);
                total.source_capacity_bytes = total
                    .source_capacity_bytes
                    .saturating_add(usage.source_capacity_bytes);
                total.current_capacity_bytes = total
                    .current_capacity_bytes
                    .saturating_add(usage.current_capacity_bytes);
                total.semantic_capacity_bytes = total
                    .semantic_capacity_bytes
                    .saturating_add(usage.semantic_capacity_bytes);
                total
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn identity(&self, key: AgentMarkdownKey) -> Option<u64> {
        match key {
            AgentMarkdownKey::Active(index) => self.active.get(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => self.history.get(&(turn_index, detail_index)),
        }
        .map(|projection| projection.identity)
    }

    #[cfg(test)]
    pub(crate) fn source(&self, key: AgentMarkdownKey) -> Option<&str> {
        match key {
            AgentMarkdownKey::Active(index) => self.active.get(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => self.history.get(&(turn_index, detail_index)),
        }
        .map(|projection| projection.document.source())
    }

    #[cfg(test)]
    pub(crate) fn projection_count(&self) -> usize {
        self.active.len().saturating_add(self.history.len())
    }

    #[cfg(test)]
    pub(crate) fn counters(&self, key: AgentMarkdownKey) -> Option<WorkCounters> {
        match key {
            AgentMarkdownKey::Active(index) => self.active.get(&index),
            AgentMarkdownKey::History {
                turn_index,
                detail_index,
            } => self.history.get(&(turn_index, detail_index)),
        }
        .map(|projection| projection.document.counters())
    }
}

fn prefixed_lines(
    rows: &[Line<'_>],
    first_row: usize,
    first_content_row: Option<usize>,
) -> Vec<Line<'static>> {
    rows.iter()
        .enumerate()
        .map(|(offset, line)| {
            if line.spans.iter().all(|span| span.content.is_empty()) {
                return Line::default();
            }
            let mut spans = Vec::with_capacity(line.spans.len() + 1);
            if first_content_row == Some(first_row + offset) {
                spans.push(Span::styled("● ", crate::theme::DOT_AGENT));
            } else {
                spans.push(Span::raw("  "));
            }
            spans.extend(line.spans.iter().map(|span| {
                Span::styled(
                    span.content.clone().into_owned(),
                    crate::theme::AGENT_TEXT.patch(line.style).patch(span.style),
                )
            }));
            Line::from(spans)
        })
        .collect()
}
