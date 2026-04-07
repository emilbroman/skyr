pub type Tag = u16;

pub enum Block {
    /// Literal text fragment (must not contain newlines).
    Literal(String),
    /// Forced line break (always emitted regardless of folding).
    Newline,
    /// Flat concatenation of blocks (no fold/unfold semantics).
    Seq(Vec<Block>),
    /// A group containing potential unfold points, rendered width-aware.
    Group(Vec<GroupItem>),
    /// Indent all content by one level (for forced indentation).
    Indent(Box<Block>),
    /// A delimited comma-separated list (records, dicts, lists, params, type params).
    CommaSep(CommaSepBlock),
}

pub enum GroupItem {
    /// Inline block content.
    Block(Block),
    /// When folded: renders as space (if space_when_folded) or nothing, plus children inline.
    /// When unfolded: newline + optional indent, children rendered at new indent level.
    PotentialUnfold {
        tag: Tag,
        space_when_folded: bool,
        indent_children: bool,
        children: Vec<Block>,
    },
}

pub struct CommaSepBlock {
    pub open: &'static str,
    pub close: &'static str,
    pub items: Vec<CommaSepItem>,
    pub space_around: bool,
    /// When true, always render unfolded regardless of width.
    /// Set when the user wrote the opening delimiter on a separate line from the first item.
    pub force_unfolded: bool,
}

pub struct CommaSepItem {
    pub leading_comments: Vec<Block>,
    pub doc_comment: Option<String>,
    pub content: Block,
    pub trailing_comment: Option<String>,
}

impl Block {
    /// Calculate the width of this block when rendered on a single line (folded).
    /// Returns `None` if the block cannot be folded (contains forced newlines).
    pub fn folded_width(&self) -> Option<usize> {
        match self {
            Block::Literal(s) => Some(s.len()),
            Block::Newline => None,
            Block::Seq(blocks) => {
                let mut total = 0;
                for b in blocks {
                    total += b.folded_width()?;
                }
                Some(total)
            }
            Block::Group(items) => {
                let mut total = 0;
                for item in items {
                    total += item.folded_width()?;
                }
                Some(total)
            }
            Block::Indent(_) => None,
            Block::CommaSep(cs) => cs.folded_width(),
        }
    }
}

impl GroupItem {
    pub fn folded_width(&self) -> Option<usize> {
        match self {
            GroupItem::Block(b) => b.folded_width(),
            GroupItem::PotentialUnfold {
                space_when_folded,
                children,
                ..
            } => {
                let mut total = if *space_when_folded { 1 } else { 0 };
                for child in children {
                    total += child.folded_width()?;
                }
                Some(total)
            }
        }
    }
}

impl CommaSepBlock {
    pub fn folded_width(&self) -> Option<usize> {
        if self.items.is_empty() {
            return Some(self.open.len() + self.close.len());
        }
        if self.force_unfolded {
            return None;
        }
        let mut total = self.open.len() + self.close.len();
        if self.space_around {
            total += 2;
        }
        for (i, item) in self.items.iter().enumerate() {
            if item.has_comments() {
                return None;
            }
            total += item.content.folded_width()?;
            if i < self.items.len() - 1 {
                total += 2; // ", "
            }
        }
        Some(total)
    }
}

impl CommaSepItem {
    pub fn has_comments(&self) -> bool {
        !self.leading_comments.is_empty()
            || self.doc_comment.is_some()
            || self.trailing_comment.is_some()
    }
}
