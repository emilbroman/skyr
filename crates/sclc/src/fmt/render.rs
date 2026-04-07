use super::block::*;

pub struct Renderer {
    output: String,
    indent: usize,
    at_line_start: bool,
    column: usize,
    max_width: usize,
    tab_width: usize,
}

impl Renderer {
    pub fn new(max_width: usize, tab_width: usize) -> Self {
        Renderer {
            output: String::new(),
            indent: 0,
            at_line_start: true,
            column: 0,
            max_width,
            tab_width,
        }
    }

    pub fn into_output(self) -> String {
        self.output
    }

    /// The column where the next character would be written, accounting for indent.
    fn effective_column(&self) -> usize {
        if self.at_line_start {
            self.indent * self.tab_width
        } else {
            self.column
        }
    }

    fn write(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.at_line_start {
            for _ in 0..self.indent {
                self.output.push('\t');
            }
            self.column = self.indent * self.tab_width;
            self.at_line_start = false;
        }
        self.output.push_str(s);
        self.column += s.len();
    }

    fn newline(&mut self) {
        self.output.push('\n');
        self.at_line_start = true;
        self.column = 0;
    }

    pub fn render(&mut self, block: &Block) {
        match block {
            Block::Literal(s) => self.write(s),
            Block::Newline => self.newline(),
            Block::Seq(blocks) => {
                for b in blocks {
                    self.render(b);
                }
            }
            Block::Group(items) => self.render_group(items),
            Block::Indent(b) => {
                self.indent += 1;
                self.render(b);
                self.indent -= 1;
            }
            Block::CommaSep(cs) => self.render_comma_sep(cs),
        }
    }

    fn render_group(&mut self, items: &[GroupItem]) {
        // Step 1: Try fully folded
        if let Some(fw) = group_folded_width(items)
            && self.effective_column() + fw <= self.max_width
        {
            self.render_group_folded(items);
            return;
        }

        // Step 2: Collect distinct tags, sorted ascending
        let mut tags: Vec<Tag> = items
            .iter()
            .filter_map(|item| {
                if let GroupItem::PotentialUnfold { tag, .. } = item {
                    Some(*tag)
                } else {
                    None
                }
            })
            .collect();
        tags.sort();
        tags.dedup();

        if tags.is_empty() {
            self.render_group_folded(items);
            return;
        }

        // Step 3: Try thresholds from highest tag downward
        for &threshold in tags.iter().rev() {
            if let Some(flw) = first_line_width(items, threshold)
                && self.effective_column() + flw <= self.max_width
            {
                self.render_group_with_threshold(items, threshold);
                return;
            }
        }

        // Step 4: Unfold everything (use lowest tag)
        self.render_group_with_threshold(items, tags[0]);
    }

    fn render_group_folded(&mut self, items: &[GroupItem]) {
        for item in items {
            match item {
                GroupItem::Block(b) => self.render(b),
                GroupItem::PotentialUnfold {
                    space_when_folded,
                    children,
                    ..
                } => {
                    if *space_when_folded {
                        self.write(" ");
                    }
                    for child in children {
                        self.render(child);
                    }
                }
            }
        }
    }

    fn render_group_with_threshold(&mut self, items: &[GroupItem], threshold: Tag) {
        // Track whether the previous item was an unfolded indent that needs
        // a newline before the next Block item to return to the base indent.
        let mut need_dedent_newline = false;

        for item in items {
            match item {
                GroupItem::Block(b) => {
                    if need_dedent_newline {
                        self.newline();
                        need_dedent_newline = false;
                    }
                    self.render(b);
                }
                GroupItem::PotentialUnfold {
                    tag,
                    space_when_folded,
                    indent_children,
                    children,
                } => {
                    if *tag >= threshold {
                        // Unfolded: newline + optional indent
                        need_dedent_newline = false;
                        self.newline();
                        if *indent_children {
                            self.indent += 1;
                        }
                        for child in children {
                            self.render(child);
                        }
                        if *indent_children {
                            self.indent -= 1;
                            need_dedent_newline = true;
                        }
                    } else {
                        // Folded: space + inline children
                        if need_dedent_newline {
                            self.newline();
                            need_dedent_newline = false;
                        }
                        if *space_when_folded {
                            self.write(" ");
                        }
                        for child in children {
                            self.render(child);
                        }
                    }
                }
            }
        }
        // Don't emit trailing newline — parent handles it.
    }

    fn render_comma_sep(&mut self, cs: &CommaSepBlock) {
        if cs.items.is_empty() {
            self.write(cs.open);
            self.write(cs.close);
            return;
        }

        // Try folded (inline)
        if let Some(fw) = cs.folded_width()
            && self.effective_column() + fw <= self.max_width
        {
            self.write(cs.open);
            if cs.space_around {
                self.write(" ");
            }
            for (i, item) in cs.items.iter().enumerate() {
                if i > 0 {
                    self.write(", ");
                }
                self.render(&item.content);
            }
            if cs.space_around {
                self.write(" ");
            }
            self.write(cs.close);
            return;
        }

        // Hugging: when a single item is itself a non-empty CommaSep, render as
        // `open + inner_unfolded + close` to avoid double-indentation.
        // e.g. `call({` ... `})` instead of `call(\n\t{\n\t\t...\n\t},\n)`
        if cs.items.len() == 1
            && !cs.items[0].has_comments()
            && let Block::CommaSep(inner) = &cs.items[0].content
            && !inner.items.is_empty()
        {
            self.write(cs.open);
            self.render_comma_sep_unfolded(inner);
            self.write(cs.close);
            return;
        }

        // Unfolded: one item per line with trailing commas
        self.render_comma_sep_unfolded(cs);
    }

    fn render_comma_sep_unfolded(&mut self, cs: &CommaSepBlock) {
        self.write(cs.open);
        self.newline();
        self.indent += 1;
        for item in &cs.items {
            for lc in &item.leading_comments {
                self.render(lc);
                self.newline();
            }
            if let Some(doc) = &item.doc_comment {
                self.render_doc_comment(doc);
            }
            self.render(&item.content);
            self.write(",");
            if let Some(tc) = &item.trailing_comment {
                self.write(" ");
                self.write(tc);
            }
            self.newline();
        }
        self.indent -= 1;
        self.write(cs.close);
    }

    fn render_doc_comment(&mut self, doc: &str) {
        for line in doc.lines() {
            if line.is_empty() {
                self.write("///");
            } else {
                self.write("/// ");
                self.write(line);
            }
            self.newline();
        }
    }
}

fn group_folded_width(items: &[GroupItem]) -> Option<usize> {
    let mut total = 0;
    for item in items {
        total += item.folded_width()?;
    }
    Some(total)
}

/// Calculate the width of the first line when rendering a group with the given threshold.
/// Items with tag >= threshold are unfolded (cause newline). We sum widths up to the first
/// unfolded item.
fn first_line_width(items: &[GroupItem], threshold: Tag) -> Option<usize> {
    let mut width = 0;
    for item in items {
        match item {
            GroupItem::Block(b) => {
                width += b.folded_width()?;
            }
            GroupItem::PotentialUnfold {
                tag,
                space_when_folded,
                children,
                ..
            } => {
                if *tag >= threshold {
                    break; // This item unfolds — first line ends here
                }
                if *space_when_folded {
                    width += 1;
                }
                for child in children {
                    width += child.folded_width()?;
                }
            }
        }
    }
    Some(width)
}
