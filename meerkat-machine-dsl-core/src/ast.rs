use proc_macro2::Span;
use syn::Ident;

/// Top-level machine definition parsed from the DSL.
#[derive(Debug)]
pub struct MachineDef {
    pub name: Ident,
    pub version: u32,
    pub rust_crate: String,
    pub rust_module: String,
    pub state_fields: Vec<FieldDef>,
    pub init_phase: Ident,
    pub init_fields: Vec<InitFieldDef>,
    pub terminal_phases: Vec<Ident>,
    pub phase_enum: PhaseEnumDef,
    pub phase_projection: Option<PhaseProjectionDef>,
    pub inputs: EnumDef,
    pub surface_only_inputs: Vec<Ident>,
    pub signals: EnumDef,
    pub effects: EnumDef,
    pub helpers: Vec<HelperDef>,
    pub invariants: Vec<InvariantDef>,
    pub transitions: Vec<TransitionDef>,
    pub dispositions: Vec<DispositionDef>,
}

impl MachineDef {
    /// Returns the name of the phase field if this is a stored-phase machine.
    ///
    /// A stored-phase machine has a state field whose type name matches
    /// the phase enum name.
    pub fn phase_field_name(&self) -> Option<&Ident> {
        let phase_type_name = self.phase_enum.name.to_string();
        self.state_fields.iter().find_map(|f| {
            if f.ty.is_named(&phase_type_name) {
                Some(&f.name)
            } else {
                None
            }
        })
    }

    pub fn is_stored_phase(&self) -> bool {
        self.phase_field_name().is_some()
    }
}

// ---------------------------------------------------------------------------
// Field and type definitions
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct FieldDef {
    pub name: Ident,
    pub ty: TypeDef,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum TypeDef {
    Bool,
    U32,
    U64,
    String,
    Option(Box<TypeDef>),
    Set(Box<TypeDef>),
    Map(Box<TypeDef>, Box<TypeDef>),
    Named(Ident),
    /// Enum type reference (maps to TypeRef::Enum in schema)
    Enum(Ident),
}

impl TypeDef {
    pub fn is_named(&self, name: &str) -> bool {
        matches!(self, TypeDef::Named(ident) if ident == name)
            || matches!(self, TypeDef::Enum(ident) if ident == name)
    }
}

#[derive(Debug)]
pub struct InitFieldDef {
    pub name: Ident,
    pub value: ExprDef,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Phase definitions
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PhaseEnumDef {
    pub name: Ident,
    pub variants: Vec<Ident>,
}

#[derive(Debug)]
pub struct PhaseProjectionDef {
    pub rules: Vec<PhaseProjectionRule>,
}

#[derive(Debug)]
pub struct PhaseProjectionRule {
    pub phase: Ident,
    /// `None` means this is the fallback (last rule, always matches).
    pub condition: Option<ExprDef>,
}

// ---------------------------------------------------------------------------
// Enum definitions (inputs, signals, effects)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct EnumDef {
    pub name: Ident,
    pub variants: Vec<VariantDef>,
}

#[derive(Debug)]
pub struct VariantDef {
    pub name: Ident,
    pub fields: Vec<FieldDef>,
}

// ---------------------------------------------------------------------------
// Helpers and invariants
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct HelperDef {
    pub name: Ident,
    pub params: Vec<FieldDef>,
    pub return_ty: TypeDef,
    pub body: ExprDef,
}

#[derive(Debug)]
pub struct InvariantDef {
    pub name: Ident,
    pub expr: ExprDef,
}

// ---------------------------------------------------------------------------
// Transitions
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct TransitionDef {
    pub name: Ident,
    pub trigger: TriggerDef,
    pub guard: Option<ExprDef>,
    pub updates: Vec<UpdateDef>,
    pub to_phase: Ident,
    pub effects: Vec<EffectEmitDef>,
    pub span: Span,
}

#[derive(Debug)]
pub enum TriggerKindDef {
    Input,
    Signal,
}

#[derive(Debug)]
pub struct TriggerDef {
    pub kind: TriggerKindDef,
    pub variant: Ident,
    pub bindings: Vec<Ident>,
}

#[derive(Debug)]
pub struct EffectEmitDef {
    pub variant: Ident,
    pub fields: Vec<(Ident, ExprDef)>,
}

// ---------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ExprDef {
    // Literals
    Bool(bool),
    U64(u64),
    StringLit(String),
    None,
    Some(Box<ExprDef>),
    EmptySet,
    EmptyMap,

    // References
    Field(Ident),
    Binding(Ident),
    CurrentPhase,
    Phase(Ident),
    /// Typed enum variant literal: `EnumName::Variant`
    NamedVariant {
        enum_name: Ident,
        variant: Ident,
    },

    // Boolean operators
    Not(Box<ExprDef>),
    And(Vec<ExprDef>),
    Or(Vec<ExprDef>),

    // Comparisons
    Eq(Box<ExprDef>, Box<ExprDef>),
    Neq(Box<ExprDef>, Box<ExprDef>),
    Gt(Box<ExprDef>, Box<ExprDef>),
    Gte(Box<ExprDef>, Box<ExprDef>),
    Lt(Box<ExprDef>, Box<ExprDef>),
    Lte(Box<ExprDef>, Box<ExprDef>),

    // Arithmetic
    Add(Box<ExprDef>, Box<ExprDef>),
    Sub(Box<ExprDef>, Box<ExprDef>),

    // Collection operations
    Contains {
        collection: Box<ExprDef>,
        value: Box<ExprDef>,
    },
    Len(Box<ExprDef>),
    MapGet {
        map: Box<ExprDef>,
        key: Box<ExprDef>,
    },
    MapKeys(Box<ExprDef>),

    // Quantifiers
    ForAll {
        binding: Ident,
        over: Box<ExprDef>,
        body: Box<ExprDef>,
    },
    Exists {
        binding: Ident,
        over: Box<ExprDef>,
        body: Box<ExprDef>,
    },

    // Method-style calls
    IsSome(Box<ExprDef>),
    IsNone(Box<ExprDef>),

    // Helper call
    Call {
        helper: Ident,
        args: Vec<ExprDef>,
    },

    // Conditional
    IfElse {
        condition: Box<ExprDef>,
        then_expr: Box<ExprDef>,
        else_expr: Box<ExprDef>,
    },
}

// ---------------------------------------------------------------------------
// Updates
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum UpdateDef {
    Assign {
        field: Ident,
        value: ExprDef,
    },
    Increment {
        field: Ident,
        amount: ExprDef,
    },
    Decrement {
        field: Ident,
        amount: ExprDef,
    },
    SetInsert {
        field: Ident,
        value: ExprDef,
    },
    SetRemove {
        field: Ident,
        value: ExprDef,
    },
    MapInsert {
        field: Ident,
        key: ExprDef,
        value: ExprDef,
    },
    MapRemove {
        field: Ident,
        key: ExprDef,
    },
    Conditional {
        condition: ExprDef,
        then_updates: Vec<UpdateDef>,
        else_updates: Vec<UpdateDef>,
    },
}

// ---------------------------------------------------------------------------
// Effect dispositions
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum DispositionKind {
    Local,
    External,
    Routed(Vec<Ident>),
}

#[derive(Debug)]
pub struct DispositionDef {
    pub effect: Ident,
    pub kind: DispositionKind,
}
