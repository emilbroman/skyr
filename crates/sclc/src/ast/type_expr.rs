use super::{TypeExpr, Var};
use crate::Loc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeDef {
    pub doc_comment: Option<String>,
    pub var: Loc<Var>,
    pub type_params: Vec<TypeParam>,
    pub ty: Loc<TypeExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeParam {
    pub var: Loc<Var>,
    pub bound: Option<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypePropertyAccessExpr {
    pub expr: Box<Loc<TypeExpr>>,
    pub property: Loc<Var>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeApplicationExpr {
    pub base: Box<Loc<TypeExpr>>,
    pub args: Vec<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnTypeExpr {
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Loc<TypeExpr>>,
    pub ret: Box<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordTypeExpr {
    pub fields: Vec<RecordTypeFieldExpr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DictTypeExpr {
    pub key: Box<Loc<TypeExpr>>,
    pub value: Box<Loc<TypeExpr>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordTypeFieldExpr {
    pub var: Loc<Var>,
    pub ty: Loc<TypeExpr>,
}
