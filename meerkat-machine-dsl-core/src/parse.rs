use proc_macro2::TokenStream;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Ident, LitInt, LitStr, Result, Token, braced, bracketed, token};

use crate::ast::*;

/// Parse the full `machine! { ... }` input.
pub fn parse_machine(input: TokenStream) -> Result<MachineDef> {
    syn::parse2::<MachineDef>(input)
}

// ---------------------------------------------------------------------------
// Top-level: machine Name { ... }
// ---------------------------------------------------------------------------

impl Parse for MachineDef {
    fn parse(input: ParseStream) -> Result<Self> {
        let kw: Ident = input.parse()?;
        if kw != "machine" {
            return Err(syn::Error::new(kw.span(), "expected `machine`"));
        }

        let name: Ident = input.parse()?;
        let content;
        braced!(content in input);

        let mut version = None;
        let mut rust_crate = None;
        let mut rust_module = None;
        let mut state_fields = None;
        let mut init_phase = None;
        let mut init_fields = None;
        let mut terminal_phases = Vec::new();
        let mut phase_enum = None;
        let mut phase_projection = None;
        let mut inputs = None;
        let mut surface_only_inputs = Vec::new();
        let mut signals = None;
        let mut effects = None;
        let mut helpers = Vec::new();
        let mut invariants = Vec::new();
        let mut transitions = Vec::new();
        let mut dispositions = Vec::new();

        while !content.is_empty() {
            let kw: Ident = content.parse()?;
            match kw.to_string().as_str() {
                "version" => {
                    let _: Token![:] = content.parse()?;
                    let lit: LitInt = content.parse()?;
                    version = Some(lit.base10_parse::<u32>()?);
                    let _: Token![,] = content.parse()?;
                }
                "rust" => {
                    let _: Token![:] = content.parse()?;
                    let crate_name: LitStr = content.parse()?;
                    let _: Token![/] = content.parse()?;
                    let module: LitStr = content.parse()?;
                    rust_crate = Some(crate_name.value());
                    rust_module = Some(module.value());
                    let _: Token![,] = content.parse()?;
                }
                "state" => {
                    state_fields = Some(parse_state_block(&content)?);
                }
                "init" => {
                    let (phase, fields) = parse_init_block(&content)?;
                    init_phase = Some(phase);
                    init_fields = Some(fields);
                }
                "terminal" => {
                    terminal_phases = parse_ident_list(&content)?;
                }
                "phase" => {
                    phase_enum = Some(parse_phase_enum(&content)?);
                }
                "phase_projection" => {
                    phase_projection = Some(parse_phase_projection(&content)?);
                }
                "input" => {
                    inputs = Some(parse_enum_def(&content)?);
                }
                "surface_only" => {
                    surface_only_inputs = parse_ident_list(&content)?;
                }
                "signal" => {
                    signals = Some(parse_enum_def(&content)?);
                }
                "effect" => {
                    effects = Some(parse_enum_def(&content)?);
                }
                "helper" => {
                    helpers.push(parse_helper(&content)?);
                }
                "invariant" => {
                    invariants.push(parse_invariant(&content)?);
                }
                "transition" => {
                    transitions.push(parse_transition(&content)?);
                }
                "disposition" => {
                    dispositions.push(parse_disposition(&content)?);
                }
                other => {
                    return Err(syn::Error::new(
                        kw.span(),
                        format!("unknown machine block keyword `{other}`"),
                    ));
                }
            }
        }

        let name_str = name.to_string();
        Ok(MachineDef {
            name,
            version: version.unwrap_or(1),
            rust_crate: rust_crate.unwrap_or_default(),
            rust_module: rust_module.unwrap_or_default(),
            state_fields: state_fields
                .ok_or_else(|| syn::Error::new_spanned(&name_str, "missing `state` block"))?,
            init_phase: init_phase
                .ok_or_else(|| syn::Error::new_spanned(&name_str, "missing `init` block"))?,
            init_fields: init_fields.unwrap_or_default(),
            terminal_phases,
            phase_enum: phase_enum
                .ok_or_else(|| syn::Error::new_spanned(&name_str, "missing `phase` block"))?,
            phase_projection,
            inputs: inputs
                .ok_or_else(|| syn::Error::new_spanned(&name_str, "missing `input` block"))?,
            surface_only_inputs,
            signals: signals.unwrap_or_else(|| EnumDef {
                name: Ident::new(&format!("{name_str}Signal"), proc_macro2::Span::call_site()),
                variants: Vec::new(),
            }),
            effects: effects.unwrap_or_else(|| EnumDef {
                name: Ident::new(&format!("{name_str}Effect"), proc_macro2::Span::call_site()),
                variants: Vec::new(),
            }),
            helpers,
            invariants,
            transitions,
            dispositions,
        })
    }
}

// ---------------------------------------------------------------------------
// state { field: Type, ... }
// ---------------------------------------------------------------------------

fn parse_state_block(input: ParseStream) -> Result<Vec<FieldDef>> {
    let content;
    braced!(content in input);
    let mut fields = Vec::new();
    while !content.is_empty() {
        fields.push(parse_field_def(&content)?);
        // Trailing comma is optional for last field
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    Ok(fields)
}

fn parse_field_def(input: ParseStream) -> Result<FieldDef> {
    let name: Ident = input.parse()?;
    let span = name.span();
    let _: Token![:] = input.parse()?;
    let ty = parse_type_def(input)?;
    Ok(FieldDef { name, ty, span })
}

fn parse_type_def(input: ParseStream) -> Result<TypeDef> {
    let ident: Ident = input.parse()?;
    match ident.to_string().as_str() {
        "bool" => Ok(TypeDef::Bool),
        "u32" => Ok(TypeDef::U32),
        "u64" => Ok(TypeDef::U64),
        "String" => Ok(TypeDef::String),
        "Option" => {
            let _: Token![<] = input.parse()?;
            let inner = parse_type_def(input)?;
            let _: Token![>] = input.parse()?;
            Ok(TypeDef::Option(Box::new(inner)))
        }
        "Set" => {
            let _: Token![<] = input.parse()?;
            let inner = parse_type_def(input)?;
            let _: Token![>] = input.parse()?;
            Ok(TypeDef::Set(Box::new(inner)))
        }
        "Map" => {
            let _: Token![<] = input.parse()?;
            let key = parse_type_def(input)?;
            let _: Token![,] = input.parse()?;
            let value = parse_type_def(input)?;
            let _: Token![>] = input.parse()?;
            Ok(TypeDef::Map(Box::new(key), Box::new(value)))
        }
        "Enum" => {
            let _: Token![<] = input.parse()?;
            let inner: Ident = input.parse()?;
            let _: Token![>] = input.parse()?;
            Ok(TypeDef::Enum(inner))
        }
        _ => Ok(TypeDef::Named(ident)),
    }
}

// ---------------------------------------------------------------------------
// init(Phase) { field = expr, ... }
// ---------------------------------------------------------------------------

fn parse_init_block(input: ParseStream) -> Result<(Ident, Vec<InitFieldDef>)> {
    let paren_content;
    syn::parenthesized!(paren_content in input);
    let phase: Ident = paren_content.parse()?;

    let content;
    braced!(content in input);
    let mut fields = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        let span = name.span();
        let _: Token![=] = content.parse()?;
        let value = parse_expr(&content)?;
        fields.push(InitFieldDef { name, value, span });
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    Ok((phase, fields))
}

// ---------------------------------------------------------------------------
// terminal [Phase1, Phase2, ...]
// ---------------------------------------------------------------------------

fn parse_ident_list(input: ParseStream) -> Result<Vec<Ident>> {
    let content;
    bracketed!(content in input);
    let idents: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
    Ok(idents.into_iter().collect())
}

// ---------------------------------------------------------------------------
// phase PhaseName { Variant1, Variant2, ... }
// ---------------------------------------------------------------------------

fn parse_phase_enum(input: ParseStream) -> Result<PhaseEnumDef> {
    let name: Ident = input.parse()?;
    let content;
    braced!(content in input);
    let variants: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
    Ok(PhaseEnumDef {
        name,
        variants: variants.into_iter().collect(),
    })
}

// ---------------------------------------------------------------------------
// phase_projection { Phase when expr, ... , Fallback }
// ---------------------------------------------------------------------------

fn parse_phase_projection(input: ParseStream) -> Result<PhaseProjectionDef> {
    let content;
    braced!(content in input);
    let mut rules = Vec::new();
    while !content.is_empty() {
        let phase: Ident = content.parse()?;
        let condition = if content.peek(Ident) {
            let when_kw: Ident = content.parse()?;
            if when_kw != "when" {
                return Err(syn::Error::new(when_kw.span(), "expected `when` or `,`"));
            }
            Some(parse_expr(&content)?)
        } else {
            None
        };
        rules.push(PhaseProjectionRule { phase, condition });
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    Ok(PhaseProjectionDef { rules })
}

// ---------------------------------------------------------------------------
// input/signal/effect EnumName { Variant { fields }, ... }
// ---------------------------------------------------------------------------

fn parse_enum_def(input: ParseStream) -> Result<EnumDef> {
    let name: Ident = input.parse()?;
    let content;
    braced!(content in input);
    let mut variants = Vec::new();
    while !content.is_empty() {
        let variant_name: Ident = content.parse()?;
        let fields = if content.peek(token::Brace) {
            let fields_content;
            braced!(fields_content in content);
            let mut fields = Vec::new();
            while !fields_content.is_empty() {
                fields.push(parse_field_def(&fields_content)?);
                if fields_content.peek(Token![,]) {
                    let _: Token![,] = fields_content.parse()?;
                }
            }
            fields
        } else {
            Vec::new()
        };
        variants.push(VariantDef {
            name: variant_name,
            fields,
        });
        if content.peek(Token![,]) {
            let _: Token![,] = content.parse()?;
        }
    }
    Ok(EnumDef { name, variants })
}

// ---------------------------------------------------------------------------
// helper name(params) -> ReturnType { body_expr }
// ---------------------------------------------------------------------------

fn parse_helper(input: ParseStream) -> Result<HelperDef> {
    let name: Ident = input.parse()?;
    let paren_content;
    syn::parenthesized!(paren_content in input);
    let mut params = Vec::new();
    while !paren_content.is_empty() {
        params.push(parse_field_def(&paren_content)?);
        if paren_content.peek(Token![,]) {
            let _: Token![,] = paren_content.parse()?;
        }
    }
    let _: Token![->] = input.parse()?;
    let return_ty = parse_type_def(input)?;
    let body_content;
    braced!(body_content in input);
    let body = parse_expr(&body_content)?;
    Ok(HelperDef {
        name,
        params,
        return_ty,
        body,
    })
}

// ---------------------------------------------------------------------------
// invariant name { expr }
// ---------------------------------------------------------------------------

fn parse_invariant(input: ParseStream) -> Result<InvariantDef> {
    let name: Ident = input.parse()?;
    let content;
    braced!(content in input);
    let expr = parse_expr(&content)?;
    Ok(InvariantDef { name, expr })
}

// ---------------------------------------------------------------------------
// transition Name { on input/signal Variant { bindings } guard { expr }
//     update { stmts } to Phase emit Effect { fields } }
// ---------------------------------------------------------------------------

fn parse_transition(input: ParseStream) -> Result<TransitionDef> {
    let name: Ident = input.parse()?;
    let span = name.span();
    let content;
    braced!(content in input);

    // optional per_phase [Phase1, Phase2, ...]
    let per_phase = if content.peek(Ident)
        && content
            .fork()
            .parse::<Ident>()
            .is_ok_and(|id| id == "per_phase")
    {
        let _kw: Ident = content.parse()?;
        Some(parse_ident_list(&content)?)
    } else {
        None
    };

    // on input/signal Variant { bindings }
    let on_kw: Ident = content.parse()?;
    if on_kw != "on" {
        return Err(syn::Error::new(on_kw.span(), "expected `on`"));
    }
    let trigger = parse_trigger(&content)?;

    // optional guard ["name"] { expr } — can repeat for multiple named guards
    let mut guards = Vec::new();
    while content.peek(Ident)
        && content
            .fork()
            .parse::<Ident>()
            .is_ok_and(|id| id == "guard")
    {
        let _guard_kw: Ident = content.parse()?;
        // optional guard name as a string literal
        let gname = if content.peek(syn::LitStr) {
            let lit: syn::LitStr = content.parse()?;
            lit.value()
        } else {
            String::new()
        };
        let guard_content;
        braced!(guard_content in content);
        guards.push(GuardDef {
            name: gname,
            expr: parse_expr(&guard_content)?,
        });
    }

    // optional update { stmts }
    let updates = if content.peek(Ident)
        && content
            .fork()
            .parse::<Ident>()
            .is_ok_and(|id| id == "update")
    {
        let _update_kw: Ident = content.parse()?;
        let update_content;
        braced!(update_content in content);
        parse_updates(&update_content)?
    } else {
        Vec::new()
    };

    // to Phase
    let to_kw: Ident = content.parse()?;
    if to_kw != "to" {
        return Err(syn::Error::new(to_kw.span(), "expected `to`"));
    }
    let to_phase: Ident = content.parse()?;

    // optional emit Effect { fields } (can repeat)
    let mut effects = Vec::new();
    while content.peek(Ident) && content.fork().parse::<Ident>().is_ok_and(|id| id == "emit") {
        let _emit_kw: Ident = content.parse()?;
        effects.push(parse_effect_emit(&content)?);
    }

    Ok(TransitionDef {
        name,
        per_phase,
        trigger,
        guards,
        updates,
        to_phase,
        effects,
        span,
    })
}

fn parse_trigger(input: ParseStream) -> Result<TriggerDef> {
    let kind_ident: Ident = input.parse()?;
    let kind = match kind_ident.to_string().as_str() {
        "input" => TriggerKindDef::Input,
        "signal" => TriggerKindDef::Signal,
        _ => {
            return Err(syn::Error::new(
                kind_ident.span(),
                "expected `input` or `signal`",
            ));
        }
    };

    let variant: Ident = input.parse()?;

    let bindings = if input.peek(token::Brace) {
        let content;
        braced!(content in input);
        let idents: Punctuated<Ident, Token![,]> = Punctuated::parse_terminated(&content)?;
        idents.into_iter().collect()
    } else {
        Vec::new()
    };

    Ok(TriggerDef {
        kind,
        variant,
        bindings,
    })
}

fn parse_effect_emit(input: ParseStream) -> Result<EffectEmitDef> {
    let variant: Ident = input.parse()?;
    let fields = if input.peek(token::Brace) {
        let content;
        braced!(content in input);
        let mut fields = Vec::new();
        while !content.is_empty() {
            let field_name: Ident = content.parse()?;
            let _: Token![:] = content.parse()?;
            let value = parse_expr(&content)?;
            fields.push((field_name, value));
            if content.peek(Token![,]) {
                let _: Token![,] = content.parse()?;
            }
        }
        fields
    } else {
        Vec::new()
    };
    Ok(EffectEmitDef { variant, fields })
}

// ---------------------------------------------------------------------------
// disposition EffectName => local | external | routed [Machine1, ...]
// ---------------------------------------------------------------------------

fn parse_disposition(input: ParseStream) -> Result<DispositionDef> {
    let effect: Ident = input.parse()?;
    let _: Token![=>] = input.parse()?;
    let kind_ident: Ident = input.parse()?;
    let kind = match kind_ident.to_string().as_str() {
        "local" => DispositionKind::Local,
        "external" => DispositionKind::External,
        "routed" => {
            let machines = parse_ident_list(input)?;
            DispositionKind::Routed(machines)
        }
        _ => {
            return Err(syn::Error::new(
                kind_ident.span(),
                "expected `local`, `external`, or `routed`",
            ));
        }
    };
    // Optional trailing comma
    if input.peek(Token![,]) {
        let _: Token![,] = input.parse()?;
    }
    Ok(DispositionDef { effect, kind })
}

// ---------------------------------------------------------------------------
// Expression parser
// ---------------------------------------------------------------------------

pub(crate) fn parse_expr(input: ParseStream) -> Result<ExprDef> {
    parse_or_expr(input)
}

fn parse_or_expr(input: ParseStream) -> Result<ExprDef> {
    let mut left = parse_and_expr(input)?;
    while input.peek(Token![||]) {
        let _: Token![||] = input.parse()?;
        let right = parse_and_expr(input)?;
        left = match left {
            ExprDef::Or(mut v) => {
                v.push(right);
                ExprDef::Or(v)
            }
            other => ExprDef::Or(vec![other, right]),
        };
    }
    Ok(left)
}

fn parse_and_expr(input: ParseStream) -> Result<ExprDef> {
    let mut left = parse_comparison_expr(input)?;
    while input.peek(Token![&&]) {
        let _: Token![&&] = input.parse()?;
        let right = parse_comparison_expr(input)?;
        left = match left {
            ExprDef::And(mut v) => {
                v.push(right);
                ExprDef::And(v)
            }
            other => ExprDef::And(vec![other, right]),
        };
    }
    Ok(left)
}

fn parse_comparison_expr(input: ParseStream) -> Result<ExprDef> {
    let left = parse_additive_expr(input)?;

    if input.peek(Token![==]) {
        let _: Token![==] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Eq(Box::new(left), Box::new(right)))
    } else if input.peek(Token![!=]) {
        let _: Token![!=] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Neq(Box::new(left), Box::new(right)))
    } else if input.peek(Token![>=]) {
        let _: Token![>=] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Gte(Box::new(left), Box::new(right)))
    } else if input.peek(Token![>]) {
        let _: Token![>] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Gt(Box::new(left), Box::new(right)))
    } else if input.peek(Token![<=]) {
        let _: Token![<=] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Lte(Box::new(left), Box::new(right)))
    } else if input.peek(Token![<]) {
        let _: Token![<] = input.parse()?;
        let right = parse_additive_expr(input)?;
        Ok(ExprDef::Lt(Box::new(left), Box::new(right)))
    } else {
        Ok(left)
    }
}

fn parse_additive_expr(input: ParseStream) -> Result<ExprDef> {
    let mut left = parse_unary_expr(input)?;
    while input.peek(Token![+]) || input.peek(Token![-]) {
        if input.peek(Token![+]) {
            let _: Token![+] = input.parse()?;
            let right = parse_unary_expr(input)?;
            left = ExprDef::Add(Box::new(left), Box::new(right));
        } else {
            let _: Token![-] = input.parse()?;
            let right = parse_unary_expr(input)?;
            left = ExprDef::Sub(Box::new(left), Box::new(right));
        }
    }
    Ok(left)
}

fn parse_unary_expr(input: ParseStream) -> Result<ExprDef> {
    if input.peek(Token![!]) {
        let _: Token![!] = input.parse()?;
        let inner = parse_postfix_expr(input)?;
        Ok(ExprDef::Not(Box::new(inner)))
    } else {
        parse_postfix_expr(input)
    }
}

fn parse_postfix_expr(input: ParseStream) -> Result<ExprDef> {
    let mut expr = parse_primary_expr(input)?;

    // Handle method calls: .is_some(), .is_none(), .contains(x), .len(), .keys(), .get(k)
    while input.peek(Token![.]) {
        let _: Token![.] = input.parse()?;
        let method: Ident = input.parse()?;
        match method.to_string().as_str() {
            "is_some" => {
                let paren;
                syn::parenthesized!(paren in input);
                let _ = &paren; // empty parens
                expr = ExprDef::IsSome(Box::new(expr));
            }
            "is_none" => {
                let paren;
                syn::parenthesized!(paren in input);
                expr = ExprDef::IsNone(Box::new(expr));
            }
            "contains" => {
                let paren;
                syn::parenthesized!(paren in input);
                let value = parse_expr(&paren)?;
                expr = ExprDef::Contains {
                    collection: Box::new(expr),
                    value: Box::new(value),
                };
            }
            "contains_key" => {
                let paren;
                syn::parenthesized!(paren in input);
                let key = parse_expr(&paren)?;
                expr = ExprDef::MapContainsKey {
                    map: Box::new(expr),
                    key: Box::new(key),
                };
            }
            "len" => {
                let paren;
                syn::parenthesized!(paren in input);
                let _ = &paren;
                expr = ExprDef::Len(Box::new(expr));
            }
            "keys" => {
                let paren;
                syn::parenthesized!(paren in input);
                let _ = &paren;
                expr = ExprDef::MapKeys(Box::new(expr));
            }
            "get" => {
                let paren;
                syn::parenthesized!(paren in input);
                let key = parse_expr(&paren)?;
                expr = ExprDef::MapGet {
                    map: Box::new(expr),
                    key: Box::new(key),
                };
            }
            _ => {
                return Err(syn::Error::new(
                    method.span(),
                    format!("unknown method `{method}`"),
                ));
            }
        }
    }

    Ok(expr)
}

fn parse_primary_expr(input: ParseStream) -> Result<ExprDef> {
    // Parenthesized expression
    if input.peek(syn::token::Paren) {
        let content;
        syn::parenthesized!(content in input);
        return parse_expr(&content);
    }

    // Integer literal
    if input.peek(LitInt) {
        let lit: LitInt = input.parse()?;
        let val = lit.base10_parse::<u64>()?;
        return Ok(ExprDef::U64(val));
    }

    // String literal
    if input.peek(LitStr) {
        let lit: LitStr = input.parse()?;
        return Ok(ExprDef::StringLit(lit.value()));
    }

    // Boolean literals (keywords in Rust, not Idents)
    if input.peek(syn::LitBool) {
        let lit: syn::LitBool = input.parse()?;
        return Ok(ExprDef::Bool(lit.value()));
    }

    // self.field
    if input.peek(Token![self]) {
        let _: Token![self] = input.parse()?;
        let _: Token![.] = input.parse()?;
        let field: Ident = input.parse()?;
        if field == "phase" {
            return Ok(ExprDef::CurrentPhase);
        }
        return Ok(ExprDef::Field(field));
    }

    // if/else expression (keyword, not Ident)
    if input.peek(Token![if]) {
        let _: Token![if] = input.parse()?;
        let condition = parse_expr(input)?;
        let then_content;
        braced!(then_content in input);
        let then_expr = parse_expr(&then_content)?;
        let _: Token![else] = input.parse()?;
        let else_content;
        braced!(else_content in input);
        let else_expr = parse_expr(&else_content)?;
        return Ok(ExprDef::IfElse {
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        });
    }

    // Identifiers (not keywords)
    let ident: Ident = input.parse()?;
    match ident.to_string().as_str() {
        "None" => Ok(ExprDef::None),
        "Some" => {
            let paren;
            syn::parenthesized!(paren in input);
            let inner = parse_expr(&paren)?;
            Ok(ExprDef::Some(Box::new(inner)))
        }
        "EmptySet" => Ok(ExprDef::EmptySet),
        "EmptyMap" => Ok(ExprDef::EmptyMap),
        "Phase" => {
            let _: Token![::] = input.parse()?;
            let variant: Ident = input.parse()?;
            Ok(ExprDef::Phase(variant))
        }
        "for_all" => {
            let paren;
            syn::parenthesized!(paren in input);
            let binding: Ident = paren.parse()?;
            let _: Token![in] = paren.parse()?;
            let over = parse_expr(&paren)?;
            let _: Token![,] = paren.parse()?;
            let body = parse_expr(&paren)?;
            Ok(ExprDef::ForAll {
                binding,
                over: Box::new(over),
                body: Box::new(body),
            })
        }
        "exists" => {
            let paren;
            syn::parenthesized!(paren in input);
            let binding: Ident = paren.parse()?;
            let _: Token![in] = paren.parse()?;
            let over = parse_expr(&paren)?;
            let _: Token![,] = paren.parse()?;
            let body = parse_expr(&paren)?;
            Ok(ExprDef::Exists {
                binding,
                over: Box::new(over),
                body: Box::new(body),
            })
        }
        _ => {
            // EnumName::Variant — named variant literal
            if input.peek(Token![::]) {
                let _: Token![::] = input.parse()?;
                let variant: Ident = input.parse()?;
                return Ok(ExprDef::NamedVariant {
                    enum_name: ident,
                    variant,
                });
            }
            // Could be a helper call: name(args) or just a binding
            if input.peek(syn::token::Paren) {
                let paren;
                syn::parenthesized!(paren in input);
                let mut args = Vec::new();
                while !paren.is_empty() {
                    args.push(parse_expr(&paren)?);
                    if paren.peek(Token![,]) {
                        let _: Token![,] = paren.parse()?;
                    }
                }
                Ok(ExprDef::Call {
                    helper: ident,
                    args,
                })
            } else {
                Ok(ExprDef::Binding(ident))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Update statements: self.field = expr; self.field += 1; etc.
// ---------------------------------------------------------------------------

fn parse_updates(input: ParseStream) -> Result<Vec<UpdateDef>> {
    let mut updates = Vec::new();
    while !input.is_empty() {
        updates.push(parse_single_update(input)?);
        // Optional semicolon
        if input.peek(Token![;]) {
            let _: Token![;] = input.parse()?;
        }
    }
    Ok(updates)
}

fn parse_single_update(input: ParseStream) -> Result<UpdateDef> {
    // Conditional: if condition { updates } else { updates }
    if input.peek(Token![if]) {
        let _: Token![if] = input.parse()?;
        let condition = parse_expr(input)?;
        let then_content;
        braced!(then_content in input);
        let then_updates = parse_updates(&then_content)?;

        let else_updates = if input.peek(Token![else]) {
            let _: Token![else] = input.parse()?;
            let else_content;
            braced!(else_content in input);
            parse_updates(&else_content)?
        } else {
            Vec::new()
        };

        return Ok(UpdateDef::Conditional {
            condition,
            then_updates,
            else_updates,
        });
    }

    // self.field <op> ...
    let _: Token![self] = input.parse()?;
    let _: Token![.] = input.parse()?;
    let field: Ident = input.parse()?;

    // Check for method-style updates: self.set.insert(v), self.map.insert(k, v), etc.
    if input.peek(Token![.]) {
        let _: Token![.] = input.parse()?;
        let method: Ident = input.parse()?;
        let paren;
        syn::parenthesized!(paren in input);

        return match method.to_string().as_str() {
            "insert" => {
                let first = parse_expr(&paren)?;
                if paren.peek(Token![,]) {
                    // map.insert(key, value)
                    let _: Token![,] = paren.parse()?;
                    let second = parse_expr(&paren)?;
                    Ok(UpdateDef::MapInsert {
                        field,
                        key: first,
                        value: second,
                    })
                } else {
                    // set.insert(value)
                    Ok(UpdateDef::SetInsert {
                        field,
                        value: first,
                    })
                }
            }
            "remove" => {
                let arg = parse_expr(&paren)?;
                // Ambiguous: could be set.remove or map.remove. We'll use the
                // field type during validation to disambiguate. For now, treat
                // single-arg remove as set remove, since map remove also takes
                // a single key.
                Ok(UpdateDef::MapRemove { field, key: arg })
            }
            "increment" => {
                let key = parse_expr(&paren)?;
                let _: Token![,] = paren.parse()?;
                let amount = parse_expr(&paren)?;
                Ok(UpdateDef::MapIncrement { field, key, amount })
            }
            "decrement" => {
                let key = parse_expr(&paren)?;
                let _: Token![,] = paren.parse()?;
                let amount = parse_expr(&paren)?;
                Ok(UpdateDef::MapDecrement { field, key, amount })
            }
            _ => Err(syn::Error::new(
                method.span(),
                format!("unknown update method `{method}`"),
            )),
        };
    }

    if input.peek(Token![+=]) {
        let _: Token![+=] = input.parse()?;
        let amount = parse_expr(input)?;
        Ok(UpdateDef::Increment { field, amount })
    } else if input.peek(Token![-=]) {
        let _: Token![-=] = input.parse()?;
        let amount = parse_expr(input)?;
        Ok(UpdateDef::Decrement { field, amount })
    } else {
        let _: Token![=] = input.parse()?;
        let value = parse_expr(input)?;
        Ok(UpdateDef::Assign { field, value })
    }
}
