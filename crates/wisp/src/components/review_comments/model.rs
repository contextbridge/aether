use super::CommentAnchor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReviewComment<A> {
    pub anchor: CommentAnchor<A>,
    pub body: String,
}

impl<A> ReviewComment<A> {
    pub(crate) fn new(anchor: CommentAnchor<A>, body: impl Into<String>) -> Self {
        Self { anchor, body: body.into() }
    }
}
