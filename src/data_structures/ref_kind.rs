/// What kind of ref a decoration chip shows; drives its color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}
