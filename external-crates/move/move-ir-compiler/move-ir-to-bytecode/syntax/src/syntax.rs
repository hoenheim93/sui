// Copyright (c) The Diem Core Contributors
// Copyright (c) The Move Contributors
// SPDX-License-Identifier: Apache-2.0

use anyhow::{anyhow, Context};
use std::{collections::BTreeSet, fmt, str::FromStr};

use crate::lexer::*;
use move_command_line_common::files::FileHash;
use move_core_types::{account_address::AccountAddress, u256};
use move_ir_types::{ast::*, location::*, spec_language_ast::*};
use move_symbol_pool::Symbol;

// FIXME: The following simplified version of ParseError copied from
// lalrpop-util should be replaced.

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ParseError<L, E> {
    InvalidToken { location: L, message: String },
    User { location: L, error: E },
}

impl<L, E> fmt::Display for ParseError<L, E>
where
    L: fmt::Display,
    E: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use self::ParseError::*;
        match *self {
            User { ref error, .. } => write!(f, "{}", error),
            InvalidToken {
                ref location,
                ref message,
            } => write!(f, "Invalid token at {}: {}", location, message),
        }
    }
}

fn make_loc(file_hash: FileHash, start: usize, end: usize) -> Loc {
    Loc::new(file_hash, start as u32, end as u32)
}

fn current_token_loc(tokens: &Lexer) -> Loc {
    let start_loc = tokens.start_loc();
    make_loc(
        tokens.file_hash(),
        start_loc,
        start_loc + tokens.content().len(),
    )
}

fn spanned<T>(file_hash: FileHash, start: usize, end: usize, value: T) -> Spanned<T> {
    Spanned {
        loc: make_loc(file_hash, start, end),
        value,
    }
}

// Check for the specified token and consume it if it matches.
// Returns true if the token matches.
fn match_token(tokens: &mut Lexer, tok: Tok) -> Result<bool, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() == tok {
        tokens.advance()?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn consume_token(tokens: &mut Lexer, tok: Tok) -> Result<(), ParseError<Loc, anyhow::Error>> {
    if tokens.peek() != tok {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: format!("expected {:?}, not {:?}", tok, tokens.peek()),
        });
    }
    tokens.advance()?;
    Ok(())
}

fn adjust_token(
    tokens: &mut Lexer,
    list_end_tokens: &[Tok],
) -> Result<(), ParseError<Loc, anyhow::Error>> {
    if tokens.peek() == Tok::GreaterGreater && list_end_tokens.contains(&Tok::Greater) {
        tokens.replace_token(Tok::Greater, 1)?;
    }
    Ok(())
}

fn parse_comma_list<F, R>(
    tokens: &mut Lexer,
    list_end_tokens: &[Tok],
    parse_list_item: F,
    allow_trailing_comma: bool,
) -> Result<Vec<R>, ParseError<Loc, anyhow::Error>>
where
    F: Fn(&mut Lexer) -> Result<R, ParseError<Loc, anyhow::Error>>,
{
    let mut v = vec![];
    adjust_token(tokens, list_end_tokens)?;
    if !list_end_tokens.contains(&tokens.peek()) {
        loop {
            v.push(parse_list_item(tokens)?);
            adjust_token(tokens, list_end_tokens)?;
            if list_end_tokens.contains(&tokens.peek()) {
                break;
            }
            consume_token(tokens, Tok::Comma)?;
            adjust_token(tokens, list_end_tokens)?;
            if list_end_tokens.contains(&tokens.peek()) && allow_trailing_comma {
                break;
            }
        }
    }
    Ok(v)
}

fn parse_list<C, F, R>(
    tokens: &mut Lexer,
    mut parse_list_continue: C,
    parse_list_item: F,
) -> Result<Vec<R>, ParseError<Loc, anyhow::Error>>
where
    C: FnMut(&mut Lexer) -> Result<bool, ParseError<Loc, anyhow::Error>>,
    F: Fn(&mut Lexer) -> Result<R, ParseError<Loc, anyhow::Error>>,
{
    let mut v = vec![];
    loop {
        v.push(parse_list_item(tokens)?);
        if !parse_list_continue(tokens)? {
            break Ok(v);
        }
    }
}

fn parse_name(tokens: &mut Lexer) -> Result<Symbol, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() != Tok::NameValue {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "expected Tok::NameValue".to_string(),
        });
    }
    let name = tokens.content();
    tokens.advance()?;
    Ok(Symbol::from(name))
}

fn parse_name_begin_ty(tokens: &mut Lexer) -> Result<Symbol, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() != Tok::NameBeginTyValue {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "expected Tok::NameBeginTyValue".to_string(),
        });
    }
    let s = tokens.content();
    // The token includes a "<" at the end, so chop that off to get the name.
    let name = &s[..s.len() - 1];
    tokens.advance()?;
    Ok(Symbol::from(name))
}

fn parse_dot_name<'input>(
    tokens: &mut Lexer<'input>,
) -> Result<&'input str, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() != Tok::DotNameValue {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "expected Tok::DotNameValue".to_string(),
        });
    }
    let name = tokens.content();
    tokens.advance()?;
    Ok(name)
}

// AccountAddress: AccountAddress = {
//     < s: r"0[xX][0-9a-fA-F]+" > => { ... }
// };

fn parse_account_address(
    tokens: &mut Lexer,
) -> Result<AccountAddress, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() != Tok::AccountAddressValue {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "expected Tok::AccountAddressValue".to_string(),
        });
    }
    let addr = AccountAddress::from_hex_literal(tokens.content())
        .with_context(|| {
            format!(
                "The address {:?} is of invalid length. Addresses are at most 32-bytes long",
                tokens.content()
            )
        })
        .unwrap();
    tokens.advance()?;
    Ok(addr)
}

// Var: Var = {
//     <n:Name> =>? Var::parse(n),
// };

fn parse_var_(tokens: &mut Lexer) -> Result<Var_, ParseError<Loc, anyhow::Error>> {
    Ok(Var_(parse_name(tokens)?))
}

fn parse_var(tokens: &mut Lexer) -> Result<Var, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let var = parse_var_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, var))
}

// Field: Field = {
//     <n:Name> =>? parse_field(n),
// };

fn parse_field(tokens: &mut Lexer) -> Result<Field, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let f = Field_(parse_name(tokens)?);
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, f))
}

/// field-ident: name-and-type-actuals '::' field
fn parse_field_ident(tokens: &mut Lexer) -> Result<FieldIdent, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
    // For now, the lexer produces 2 ':' tokens instead of a single '::' token.
    consume_token(tokens, Tok::Colon)?;
    consume_token(tokens, Tok::Colon)?;
    let field = parse_field(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(
        tokens.file_hash(),
        start_loc,
        end_loc,
        FieldIdent_ {
            struct_name: StructName(name),
            type_actuals,
            field,
        },
    ))
}

// CopyableVal: CopyableVal = {
//     AccountAddress => CopyableVal::Address(<>),
//     "true" => CopyableVal::Bool(true),
//     "false" => CopyableVal::Bool(false),
//     <i: U64> => CopyableVal::U64(i),
//     <buf: ByteArray> => CopyableVal::ByteArray(buf),
// }

fn parse_copyable_val(tokens: &mut Lexer) -> Result<CopyableVal, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let val = match tokens.peek() {
        Tok::AccountAddressValue => {
            let addr = parse_account_address(tokens)?;
            CopyableVal_::Address(addr)
        }
        Tok::True => {
            tokens.advance()?;
            CopyableVal_::Bool(true)
        }
        Tok::False => {
            tokens.advance()?;
            CopyableVal_::Bool(false)
        }
        Tok::U8Value => {
            let mut s = tokens.content();
            if s.ends_with("u8") {
                s = &s[..s.len() - 2]
            }
            let i = u8::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U8(i)
        }
        Tok::U16Value => {
            let mut s = tokens.content();
            if s.ends_with("u16") {
                s = &s[..s.len() - 3]
            }
            let i = u16::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U16(i)
        }
        Tok::U32Value => {
            let mut s = tokens.content();
            if s.ends_with("u32") {
                s = &s[..s.len() - 3]
            }
            let i = u32::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U32(i)
        }
        Tok::U64Value => {
            let mut s = tokens.content();
            if s.ends_with("u64") {
                s = &s[..s.len() - 3]
            }
            let i = u64::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U64(i)
        }
        Tok::U128Value => {
            let mut s = tokens.content();
            if s.ends_with("u128") {
                s = &s[..s.len() - 4]
            }
            let i = u128::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U128(i)
        }
        Tok::U256Value => {
            let mut s = tokens.content();
            if s.ends_with("256") {
                s = &s[..s.len() - 4]
            }
            let i = u256::U256::from_str(s).unwrap();
            tokens.advance()?;
            CopyableVal_::U256(i)
        }
        Tok::ByteArrayValue => {
            let s = tokens.content();
            let buf = hex::decode(&s[2..s.len() - 1]).unwrap_or_else(|_| {
                // The lexer guarantees this, but tracking this knowledge all the way to here is tedious
                unreachable!("The string {:?} is not a valid hex-encoded byte array", s)
            });
            tokens.advance()?;
            CopyableVal_::ByteArray(buf)
        }
        t => {
            return Err(ParseError::InvalidToken {
                location: current_token_loc(tokens),
                message: format!("unrecognized token kind {:?}", t),
            })
        }
    };
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, val))
}

// Get the precedence of a binary operator. The minimum precedence value
// is 1, and larger values have higher precedence. For tokens that are not
// binary operators, this returns a value of zero so that they will be
// below the minimum value and will mark the end of the binary expression
// for the code in parse_rhs_of_binary_exp.
// Precedences are not sequential to make it easier to add new binops without
// renumbering everything.
fn get_precedence(token: Tok) -> u32 {
    match token {
        // Reserved minimum precedence value is 1 (specified in parse_exp_)
        // TODO
        // Tok::EqualEqualGreater may not work right,
        // since parse_spec_exp calls parse_rhs_of_spec_exp
        // with min_prec = 1.  So parse_spec_expr will stop parsing instead of reading ==>
        Tok::EqualEqualGreater => 1,
        Tok::ColonEqual => 3,
        Tok::PipePipe => 5,
        Tok::AmpAmp => 10,
        Tok::EqualEqual => 15,
        Tok::ExclaimEqual => 15,
        Tok::Less => 15,
        Tok::Greater => 15,
        Tok::LessEqual => 15,
        Tok::GreaterEqual => 15,
        Tok::PeriodPeriod => 20,
        Tok::Pipe => 25,
        Tok::Caret => 30,
        Tok::Amp => 35,
        Tok::LessLess => 40,
        Tok::GreaterGreater => 40,
        Tok::Plus => 45,
        Tok::Minus => 45,
        Tok::Star => 50,
        Tok::Slash => 50,
        Tok::Percent => 50,
        _ => 0, // anything else is not a binary operator
    }
}

fn parse_exp(tokens: &mut Lexer) -> Result<Exp, ParseError<Loc, anyhow::Error>> {
    let lhs = parse_unary_exp(tokens)?;
    parse_rhs_of_binary_exp(tokens, lhs, /* min_prec */ 1)
}

fn parse_rhs_of_binary_exp(
    tokens: &mut Lexer,
    lhs: Exp,
    min_prec: u32,
) -> Result<Exp, ParseError<Loc, anyhow::Error>> {
    let mut result = lhs;
    let mut next_tok_prec = get_precedence(tokens.peek());

    // Continue parsing binary expressions as long as they have they
    // specified minimum precedence.
    while next_tok_prec >= min_prec {
        let op_token = tokens.peek();
        tokens.advance()?;

        let mut rhs = parse_unary_exp(tokens)?;

        // If the next token is another binary operator with a higher
        // precedence, then recursively parse that expression as the RHS.
        let this_prec = next_tok_prec;
        next_tok_prec = get_precedence(tokens.peek());
        if this_prec < next_tok_prec {
            rhs = parse_rhs_of_binary_exp(tokens, rhs, this_prec + 1)?;
            next_tok_prec = get_precedence(tokens.peek());
        }

        let op = match op_token {
            Tok::EqualEqual => BinOp::Eq,
            Tok::ExclaimEqual => BinOp::Neq,
            Tok::Less => BinOp::Lt,
            Tok::Greater => BinOp::Gt,
            Tok::LessEqual => BinOp::Le,
            Tok::GreaterEqual => BinOp::Ge,
            Tok::PipePipe => BinOp::Or,
            Tok::AmpAmp => BinOp::And,
            Tok::Caret => BinOp::Xor,
            Tok::LessLess => BinOp::Shl,
            Tok::GreaterGreater => BinOp::Shr,
            Tok::Pipe => BinOp::BitOr,
            Tok::Amp => BinOp::BitAnd,
            Tok::Plus => BinOp::Add,
            Tok::Minus => BinOp::Sub,
            Tok::Star => BinOp::Mul,
            Tok::Slash => BinOp::Div,
            Tok::Percent => BinOp::Mod,
            _ => panic!("Unexpected token that is not a binary operator"),
        };
        let start_loc = result.loc.start();
        let end_loc = tokens.previous_end_loc();
        let e = Exp_::BinopExp(Box::new(result), op, Box::new(rhs));
        result = spanned(tokens.file_hash(), start_loc as usize, end_loc, e);
    }

    Ok(result)
}

// QualifiedFunctionName : FunctionCall = {
//     <f: Builtin> => FunctionCall::Builtin(f),
//     <module_dot_name: DotName> <type_actuals: TypeActuals> =>? { ... }
// }

fn parse_qualified_function_name(
    tokens: &mut Lexer,
) -> Result<FunctionCall, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let call = match tokens.peek() {
        Tok::Exists
        | Tok::BorrowGlobal
        | Tok::BorrowGlobalMut
        | Tok::MoveFrom
        | Tok::MoveTo
        | Tok::VecPack(_)
        | Tok::VecLen
        | Tok::VecImmBorrow
        | Tok::VecMutBorrow
        | Tok::VecPushBack
        | Tok::VecPopBack
        | Tok::VecUnpack(_)
        | Tok::VecSwap
        | Tok::Freeze
        | Tok::ToU8
        | Tok::ToU16
        | Tok::ToU32
        | Tok::ToU64
        | Tok::ToU128
        | Tok::ToU256 => {
            let f = parse_builtin(tokens)?;
            FunctionCall_::Builtin(f)
        }
        Tok::DotNameValue => {
            let module_dot_name = parse_dot_name(tokens)?;
            let type_actuals = parse_type_actuals(tokens)?;
            let v: Vec<&str> = module_dot_name.split('.').collect();
            assert!(v.len() == 2);
            FunctionCall_::ModuleFunctionCall {
                module: ModuleName(Symbol::from(v[0])),
                name: FunctionName(Symbol::from(v[1])),
                type_actuals,
            }
        }
        t => {
            return Err(ParseError::InvalidToken {
                location: current_token_loc(tokens),
                message: format!(
                    "unrecognized token kind for qualified function name {:?}",
                    t
                ),
            })
        }
    };
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, call))
}

// UnaryExp : Exp = {
//     "!" <e: Sp<UnaryExp>> => Exp::UnaryExp(UnaryOp::Not, Box::new(e)),
//     "*" <e: Sp<UnaryExp>> => Exp::Dereference(Box::new(e)),
//     "&mut " <e: Sp<UnaryExp>> "." <f: Field> => { ... },
//     "&" <e: Sp<UnaryExp>> "." <f: Field> => { ... },
//     CallOrTerm,
// }

fn parse_borrow_field_(
    tokens: &mut Lexer,
    mutable: bool,
) -> Result<Exp_, ParseError<Loc, anyhow::Error>> {
    // This could be either a field borrow (from UnaryExp) or
    // a borrow of a local variable (from Term). In the latter case,
    // only a simple name token is allowed, and it must not be
    // the start of a pack expression.
    let e = if tokens.peek() == Tok::NameValue {
        if tokens.lookahead()? != Tok::LBrace {
            let var = parse_var(tokens)?;
            return Ok(Exp_::BorrowLocal(mutable, var));
        }
        let start_loc = tokens.start_loc();
        let name = parse_name(tokens)?;
        let end_loc = tokens.previous_end_loc();
        let type_actuals: Vec<Type> = vec![];
        spanned(
            tokens.file_hash(),
            start_loc,
            end_loc,
            parse_pack_(tokens, name, type_actuals)?,
        )
    } else {
        parse_unary_exp(tokens)?
    };
    consume_token(tokens, Tok::Period)?;
    let field = parse_field_ident(tokens)?;
    Ok(Exp_::Borrow {
        is_mutable: mutable,
        exp: Box::new(e),
        field,
    })
}

fn parse_unary_exp_(tokens: &mut Lexer) -> Result<Exp_, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Exclaim => {
            tokens.advance()?;
            let e = parse_unary_exp(tokens)?;
            Ok(Exp_::UnaryExp(UnaryOp::Not, Box::new(e)))
        }
        Tok::Star => {
            tokens.advance()?;
            let e = parse_unary_exp(tokens)?;
            Ok(Exp_::Dereference(Box::new(e)))
        }
        Tok::AmpMut => {
            tokens.advance()?;
            parse_borrow_field_(tokens, true)
        }
        Tok::Amp => {
            tokens.advance()?;
            parse_borrow_field_(tokens, false)
        }
        _ => parse_call_or_term_(tokens),
    }
}

fn parse_unary_exp(tokens: &mut Lexer) -> Result<Exp, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let e = parse_unary_exp_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, e))
}

// Call: Exp = {
//     <f: Sp<QualifiedFunctionName>> <exp: Sp<CallOrTerm>> => Exp::FunctionCall(f, Box::new(exp)),
// }

fn parse_call(tokens: &mut Lexer) -> Result<Exp, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let f = parse_qualified_function_name(tokens)?;
    let exp = parse_call_or_term(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(
        tokens.file_hash(),
        start_loc,
        end_loc,
        Exp_::FunctionCall(f, Box::new(exp)),
    ))
}

// CallOrTerm: Exp = {
//     <f: Sp<QualifiedFunctionName>> <exp: Sp<CallOrTerm>> => Exp::FunctionCall(f, Box::new(exp)),
//     Term,
// }

fn parse_call_or_term_(tokens: &mut Lexer) -> Result<Exp_, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Exists
        | Tok::BorrowGlobal
        | Tok::BorrowGlobalMut
        | Tok::MoveFrom
        | Tok::MoveTo
        | Tok::VecPack(_)
        | Tok::VecLen
        | Tok::VecImmBorrow
        | Tok::VecMutBorrow
        | Tok::VecPushBack
        | Tok::VecPopBack
        | Tok::VecUnpack(_)
        | Tok::VecSwap
        | Tok::Freeze
        | Tok::DotNameValue
        | Tok::ToU8
        | Tok::ToU16
        | Tok::ToU32
        | Tok::ToU64
        | Tok::ToU128
        | Tok::ToU256 => {
            let f = parse_qualified_function_name(tokens)?;
            let exp = parse_call_or_term(tokens)?;
            Ok(Exp_::FunctionCall(f, Box::new(exp)))
        }
        _ => parse_term_(tokens),
    }
}

fn parse_call_or_term(tokens: &mut Lexer) -> Result<Exp, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let v = parse_call_or_term_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, v))
}

// FieldExp: (Field_, Exp_) = {
//     <f: Sp<Field>> ":" <e: Sp<Exp>> => (f, e)
// }

fn parse_field_exp(tokens: &mut Lexer) -> Result<(Field, Exp), ParseError<Loc, anyhow::Error>> {
    let f = parse_field(tokens)?;
    consume_token(tokens, Tok::Colon)?;
    let e = parse_exp(tokens)?;
    Ok((f, e))
}

// Term: Exp = {
//     "move(" <v: Sp<Var>> ")" => Exp::Move(v),
//     "copy(" <v: Sp<Var>> ")" => Exp::Copy(v),
//     "&mut " <v: Sp<Var>> => Exp::BorrowLocal(true, v),
//     "&" <v: Sp<Var>> => Exp::BorrowLocal(false, v),
//     Sp<CopyableVal> => Exp::Value(<>),
//     <name_and_type_actuals: NameAndTypeActuals> "{" <fs:Comma<FieldExp>> "}" =>? { ... },
//     "(" <exps: Comma<Sp<Exp>>> ")" => Exp::ExprList(exps),
// }

fn parse_pack_(
    tokens: &mut Lexer,
    name: Symbol,
    type_actuals: Vec<Type>,
) -> Result<Exp_, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::LBrace)?;
    let fs = parse_comma_list(tokens, &[Tok::RBrace], parse_field_exp, true)?;
    consume_token(tokens, Tok::RBrace)?;
    Ok(Exp_::Pack(
        StructName(name),
        type_actuals,
        fs.into_iter().collect::<Vec<_>>(),
    ))
}

fn parse_term_(tokens: &mut Lexer) -> Result<Exp_, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Move => {
            tokens.advance()?;
            let v = parse_var(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            Ok(Exp_::Move(v))
        }
        Tok::Copy => {
            tokens.advance()?;
            let v = parse_var(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            Ok(Exp_::Copy(v))
        }
        Tok::AmpMut => {
            tokens.advance()?;
            let v = parse_var(tokens)?;
            Ok(Exp_::BorrowLocal(true, v))
        }
        Tok::Amp => {
            tokens.advance()?;
            let v = parse_var(tokens)?;
            Ok(Exp_::BorrowLocal(false, v))
        }
        Tok::AccountAddressValue
        | Tok::True
        | Tok::False
        | Tok::U8Value
        | Tok::U16Value
        | Tok::U32Value
        | Tok::U64Value
        | Tok::U128Value
        | Tok::U256Value
        | Tok::ByteArrayValue => Ok(Exp_::Value(parse_copyable_val(tokens)?)),
        Tok::NameValue | Tok::NameBeginTyValue => {
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            parse_pack_(tokens, name, type_actuals)
        }
        Tok::LParen => {
            tokens.advance()?;
            let exps = parse_comma_list(tokens, &[Tok::RParen], parse_exp, true)?;
            consume_token(tokens, Tok::RParen)?;
            Ok(Exp_::ExprList(exps))
        }
        t => Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: format!("unrecognized token kind for term {:?}", t),
        }),
    }
}

// StructName: StructName = {
//     <n: Name> =>? StructName::parse(n),
// }

fn parse_struct_name(tokens: &mut Lexer) -> Result<StructName, ParseError<Loc, anyhow::Error>> {
    Ok(StructName(parse_name(tokens)?))
}

// QualifiedStructIdent : QualifiedStructIdent = {
//     <module_dot_struct: DotName> =>? { ... }
// }

fn parse_qualified_struct_ident(
    tokens: &mut Lexer,
) -> Result<QualifiedStructIdent, ParseError<Loc, anyhow::Error>> {
    let module_dot_struct = parse_dot_name(tokens)?;
    let v: Vec<&str> = module_dot_struct.split('.').collect();
    assert!(v.len() == 2);
    let m: ModuleName = ModuleName(Symbol::from(v[0]));
    let n: StructName = StructName(Symbol::from(v[1]));
    Ok(QualifiedStructIdent::new(m, n))
}

// ModuleName: ModuleName = {
//     <n: Name> =>? ModuleName::parse(n),
// }

fn parse_module_name(tokens: &mut Lexer) -> Result<ModuleName, ParseError<Loc, anyhow::Error>> {
    Ok(ModuleName(parse_name(tokens)?))
}

fn consume_end_of_generics(tokens: &mut Lexer) -> Result<(), ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Greater => tokens.advance(),
        Tok::GreaterGreater => {
            tokens.replace_token(Tok::Greater, 1)?;
            tokens.advance()?;
            Ok(())
        }
        _ => Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "expected Tok::Greater or Tok::GreaterGreater".to_string(),
        }),
    }
}

// Builtin: Builtin = {
//     "exists<" <name_and_type_actuals: NameAndTypeActuals> ">" =>? { ... },
//     "borrow_global<" <name_and_type_actuals: NameAndTypeActuals> ">" =>? { ... },
//     "borrow_global_mut<" <name_and_type_actuals: NameAndTypeActuals> ">" =>? { ... },
//     "move_to<" <name_and_type_actuals: NameAndTypeActuals> ">" =>? { ... },
//     "move_from<" <name_and_type_actuals: NameAndTypeActuals> ">" =>? { ... },
//     "vec_*<" <type_actuals: TypeActuals> ">" =>? { ... },
//     "freeze" => Builtin::Freeze,
// }

fn parse_builtin(tokens: &mut Lexer) -> Result<Builtin, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Exists => {
            tokens.advance()?;
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            consume_end_of_generics(tokens)?;
            Ok(Builtin::Exists(StructName(name), type_actuals))
        }
        Tok::BorrowGlobal => {
            tokens.advance()?;
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            consume_end_of_generics(tokens)?;
            Ok(Builtin::BorrowGlobal(false, StructName(name), type_actuals))
        }
        Tok::BorrowGlobalMut => {
            tokens.advance()?;
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            consume_end_of_generics(tokens)?;
            Ok(Builtin::BorrowGlobal(true, StructName(name), type_actuals))
        }
        Tok::MoveFrom => {
            tokens.advance()?;
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            consume_end_of_generics(tokens)?;
            Ok(Builtin::MoveFrom(StructName(name), type_actuals))
        }
        Tok::MoveTo => {
            tokens.advance()?;
            let (name, type_actuals) = parse_name_and_type_actuals(tokens)?;
            consume_end_of_generics(tokens)?;
            Ok(Builtin::MoveTo(StructName(name), type_actuals))
        }
        Tok::VecPack(num) => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecPack(type_actuals, num))
        }
        Tok::VecLen => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecLen(type_actuals))
        }
        Tok::VecImmBorrow => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecImmBorrow(type_actuals))
        }
        Tok::VecMutBorrow => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecMutBorrow(type_actuals))
        }
        Tok::VecPushBack => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecPushBack(type_actuals))
        }
        Tok::VecPopBack => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecPopBack(type_actuals))
        }
        Tok::VecUnpack(num) => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecUnpack(type_actuals, num))
        }
        Tok::VecSwap => {
            tokens.advance()?;
            let type_actuals = parse_type_actuals(tokens)?;
            Ok(Builtin::VecSwap(type_actuals))
        }
        Tok::Freeze => {
            tokens.advance()?;
            Ok(Builtin::Freeze)
        }
        Tok::ToU8 => {
            tokens.advance()?;
            Ok(Builtin::ToU8)
        }
        Tok::ToU16 => {
            tokens.advance()?;
            Ok(Builtin::ToU16)
        }
        Tok::ToU32 => {
            tokens.advance()?;
            Ok(Builtin::ToU32)
        }
        Tok::ToU64 => {
            tokens.advance()?;
            Ok(Builtin::ToU64)
        }
        Tok::ToU128 => {
            tokens.advance()?;
            Ok(Builtin::ToU128)
        }
        Tok::ToU256 => {
            tokens.advance()?;
            Ok(Builtin::ToU256)
        }
        t => Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: format!("unrecognized token kind for builtin {:?}", t),
        }),
    }
}

// LValue: LValue = {
//     <l:Sp<Var>> => LValue::Var(l),
//     "*" <e: Sp<Exp>> => LValue::Mutate(e),
//     "_" => LValue::Pop,
// }

fn parse_lvalue_(tokens: &mut Lexer) -> Result<LValue_, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::NameValue => {
            let l = parse_var(tokens)?;
            Ok(LValue_::Var(l))
        }
        Tok::Star => {
            tokens.advance()?;
            let e = parse_exp(tokens)?;
            Ok(LValue_::Mutate(e))
        }
        Tok::Underscore => {
            tokens.advance()?;
            Ok(LValue_::Pop)
        }
        t => Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: format!("unrecognized token kind for lvalue {:?}", t),
        }),
    }
}

fn parse_lvalue(tokens: &mut Lexer) -> Result<LValue, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let lv = parse_lvalue_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, lv))
}

// FieldBindings: (Field_, Var_) = {
//     <f: Sp<Field>> ":" <v: Sp<Var>> => (f, v),
//     <f: Sp<Field>> => { ... }
// }

fn parse_field_bindings(
    tokens: &mut Lexer,
) -> Result<(Field, Var), ParseError<Loc, anyhow::Error>> {
    let f = parse_field(tokens)?;
    if tokens.peek() == Tok::Colon {
        tokens.advance()?; // consume the colon
        let v = parse_var(tokens)?;
        Ok((f, v))
    } else {
        Ok((
            f.clone(),
            Spanned {
                loc: f.loc,
                value: Var_(f.value.0),
            },
        ))
    }
}

// pub Cmd : Cmd = {
//     <lvalues: Comma<Sp<LValue>>> "=" <e: Sp<Exp>> => Cmd::Assign(lvalues, e),
//     <name_and_type_actuals: NameAndTypeActuals> "{" <bindings: Comma<FieldBindings>> "}" "=" <e: Sp<Exp>> =>? { ... },
//     "abort" <err: Sp<Exp>?> => { ... },
//     "return" <v: Comma<Sp<Exp>>> => Cmd::Return(Box::new(Spanned::unsafe_no_loc(Exp::ExprList(v)))),
//     "continue" => Cmd::Continue,
//     "break" => Cmd::Break,
//     <Sp<Call>> => Cmd::Exp(Box::new(<>)),
//     "(" <Comma<Sp<Exp>>> ")" => Cmd::Exp(Box::new(Spanned::unsafe_no_loc(Exp::ExprList(<>)))),
// }

fn parse_assign_(tokens: &mut Lexer) -> Result<Statement_, ParseError<Loc, anyhow::Error>> {
    let lvalues = parse_comma_list(tokens, &[Tok::Equal], parse_lvalue, false)?;
    if lvalues.is_empty() {
        return Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: "could not parse lvalues in assignment".to_string(),
        });
    }
    consume_token(tokens, Tok::Equal)?;
    let e = parse_exp(tokens)?;
    Ok(Statement_::Assign(lvalues, e))
}

fn parse_unpack_(
    tokens: &mut Lexer,
    name: Symbol,
    type_actuals: Vec<Type>,
) -> Result<Statement_, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::LBrace)?;
    let bindings = parse_comma_list(tokens, &[Tok::RBrace], parse_field_bindings, true)?;
    consume_token(tokens, Tok::RBrace)?;
    consume_token(tokens, Tok::Equal)?;
    let e = parse_exp(tokens)?;
    Ok(Statement_::Unpack(
        StructName(name),
        type_actuals,
        bindings.into_iter().collect(),
        Box::new(e),
    ))
}

/// Parses a statement.
fn parse_statement_(tokens: &mut Lexer) -> Result<Statement_, ParseError<Loc, anyhow::Error>> {
    match tokens.peek() {
        Tok::Abort => {
            tokens.advance()?;
            let val = if tokens.peek() == Tok::Semicolon {
                None
            } else {
                Some(Box::new(parse_exp(tokens)?))
            };
            Ok(Statement_::Abort(val))
        }
        Tok::Assert => {
            tokens.advance()?;
            let e = parse_exp(tokens)?;
            consume_token(tokens, Tok::Comma)?;
            let err = parse_exp(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            let cond = {
                let loc = e.loc;
                sp(loc, Exp_::UnaryExp(UnaryOp::Not, Box::new(e)))
            };
            Ok(Statement_::Assert(Box::new(cond), Box::new(err)))
        }
        Tok::Jump => {
            consume_token(tokens, Tok::Jump)?;
            Ok(Statement_::Jump(parse_label(tokens)?))
        }
        Tok::JumpIf => {
            consume_token(tokens, Tok::JumpIf)?;
            consume_token(tokens, Tok::LParen)?;
            let cond = parse_exp(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            Ok(Statement_::JumpIf(Box::new(cond), parse_label(tokens)?))
        }
        Tok::JumpIfFalse => {
            consume_token(tokens, Tok::JumpIfFalse)?;
            consume_token(tokens, Tok::LParen)?;
            let cond = parse_exp(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            Ok(Statement_::JumpIfFalse(
                Box::new(cond),
                parse_label(tokens)?,
            ))
        }
        Tok::NameValue => {
            // This could be either an LValue for an assignment or
            // NameAndTypeActuals (with no type_actuals) for an unpack.
            if tokens.lookahead()? == Tok::LBrace {
                let name = parse_name(tokens)?;
                parse_unpack_(tokens, name, vec![])
            } else {
                parse_assign_(tokens)
            }
        }
        Tok::Return => {
            tokens.advance()?;
            let start = tokens.start_loc();
            let v = parse_comma_list(tokens, &[Tok::Semicolon], parse_exp, true)?;
            let end = tokens.start_loc();
            Ok(Statement_::Return(Box::new(spanned(
                tokens.file_hash(),
                start,
                end,
                Exp_::ExprList(v),
            ))))
        }
        Tok::Star | Tok::Underscore => parse_assign_(tokens),
        Tok::NameBeginTyValue => {
            let (name, tys) = parse_name_and_type_actuals(tokens)?;
            parse_unpack_(tokens, name, tys)
        }
        Tok::Exists
        | Tok::BorrowGlobal
        | Tok::BorrowGlobalMut
        | Tok::MoveFrom
        | Tok::MoveTo
        | Tok::VecPack(_)
        | Tok::VecLen
        | Tok::VecImmBorrow
        | Tok::VecMutBorrow
        | Tok::VecPushBack
        | Tok::VecPopBack
        | Tok::VecUnpack(_)
        | Tok::VecSwap
        | Tok::Freeze
        | Tok::DotNameValue
        | Tok::ToU8
        | Tok::ToU16
        | Tok::ToU32
        | Tok::ToU64
        | Tok::ToU128
        | Tok::ToU256 => Ok(Statement_::Exp(Box::new(parse_call(tokens)?))),
        Tok::LParen => {
            tokens.advance()?;
            let start = tokens.start_loc();
            let v = parse_comma_list(tokens, &[Tok::RParen], parse_exp, true)?;
            consume_token(tokens, Tok::RParen)?;
            let end = tokens.start_loc();
            Ok(Statement_::Exp(Box::new(spanned(
                tokens.file_hash(),
                start,
                end,
                Exp_::ExprList(v),
            ))))
        }
        t => Err(ParseError::InvalidToken {
            location: current_token_loc(tokens),
            message: format!("invalid token kind for statement {:?}", t),
        }),
    }
}

/// Parses a statement with its location.
fn parse_statement(tokens: &mut Lexer) -> Result<Statement, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let c = parse_statement_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    let cmd = spanned(tokens.file_hash(), start_loc, end_loc, c);
    consume_token(tokens, Tok::Semicolon)?;
    Ok(cmd)
}

/// Parses a label declaration for a block, e.g.: `label b0:`.
fn parse_block_label(tokens: &mut Lexer) -> Result<BlockLabel, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Label)?;
    let label = parse_label(tokens)?;
    consume_token(tokens, Tok::Colon)?;
    Ok(label)
}

/// Parses a label identifier, e.g.: the `b0` in the statement `jump b0;`.
fn parse_label(tokens: &mut Lexer) -> Result<BlockLabel, ParseError<Loc, anyhow::Error>> {
    let start = tokens.start_loc();
    let name = parse_name(tokens)?;
    let end = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start, end, BlockLabel_(name)))
}

/// Parses a sequence of blocks, such as would appear within the `{` and `}` delimiters of a
/// function body.
fn parse_blocks(tokens: &mut Lexer) -> Result<Vec<Block>, ParseError<Loc, anyhow::Error>> {
    let mut blocks = vec![];
    while tokens.peek() != Tok::RBrace {
        blocks.push(parse_block(tokens)?);
    }
    Ok(blocks)
}

/// Parses a block: its block label `label b:`, and a sequence of 0 or more statements.
fn parse_block(tokens: &mut Lexer) -> Result<Block, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let label = parse_block_label(tokens)?;
    let mut statements = vec![];
    while !matches!(tokens.peek(), Tok::Label | Tok::RBrace) {
        statements.push(parse_statement(tokens)?);
    }
    Ok(spanned(
        tokens.file_hash(),
        start_loc,
        tokens.previous_end_loc(),
        Block_::new(label, statements),
    ))
}

// Declaration: (Var_, Type) = {
//   "let" <v: Sp<Var>> ":" <t: Type> ";" => (v, t),
// }

fn parse_declaration(tokens: &mut Lexer) -> Result<(Var, Type), ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Let)?;
    let v = parse_var(tokens)?;
    consume_token(tokens, Tok::Colon)?;
    let t = parse_type(tokens)?;
    consume_token(tokens, Tok::Semicolon)?;
    Ok((v, t))
}

// Declarations: Vec<(Var_, Type)> = {
//     <Declaration*>
// }

fn parse_declarations(
    tokens: &mut Lexer,
) -> Result<Vec<(Var, Type)>, ParseError<Loc, anyhow::Error>> {
    let mut decls: Vec<(Var, Type)> = vec![];
    // Declarations always begin with the "let" token so continue parsing
    // them until we hit something else.
    while tokens.peek() == Tok::Let {
        decls.push(parse_declaration(tokens)?);
    }
    Ok(decls)
}

// FunctionBlock: (Vec<(Var_, Type)>, Block) = {
//     "{" <locals: Declarations> <stmts: Statements> "}" => (locals, Block::new(stmts))
// }
#[allow(clippy::type_complexity)]
fn parse_function_block_(
    tokens: &mut Lexer,
) -> Result<(Vec<(Var, Type)>, Vec<Block>), ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::LBrace)?;
    let locals = parse_declarations(tokens)?;
    let statements = parse_blocks(tokens)?;
    consume_token(tokens, Tok::RBrace)?;
    Ok((locals, statements))
}

fn token_to_ability(token: Tok, contents: &str) -> Option<Ability> {
    match (token, contents) {
        (Tok::Copy, _) => Some(Ability::Copy),
        (Tok::NameValue, Ability::DROP) => Some(Ability::Drop),
        (Tok::NameValue, Ability::STORE) => Some(Ability::Store),
        (Tok::NameValue, Ability::KEY) => Some(Ability::Key),
        _ => None,
    }
}

// Ability: Ability = {
//     "copy" => Ability::Copy,
//     "drop" => Ability::Drop,
//     "store" => Ability::Store,
//     "key" => Ability::Key,
// }
fn parse_ability(tokens: &mut Lexer) -> Result<(Ability, Loc), ParseError<Loc, anyhow::Error>> {
    let a = match token_to_ability(tokens.peek(), tokens.content()) {
        Some(a) => (a, current_token_loc(tokens)),
        None => {
            return Err(ParseError::InvalidToken {
                location: current_token_loc(tokens),
                message: "could not parse ability".to_string(),
            })
        }
    };
    tokens.advance()?;
    Ok(a)
}

// Type: Type = {
//     "address" => Type::Address,
//     "signer" => Type::Signer,
//     "u64" => Type::U64,
//     "bool" => Type::Bool,
//     "bytearray" => Type::ByteArray,
//     <s: QualifiedStructIdent> <tys: TypeActuals> => Type::Struct(s, tys),
//     "&" <t: Type> => Type::Reference(false, Box::new(t)),
//     "&mut " <t: Type> => Type::Reference(true, Box::new(t)),
//     <n: Name> =>? Ok(Type::TypeParameter(TypeVar::parse(n)?)),
// }

fn parse_type(tokens: &mut Lexer) -> Result<Type, ParseError<Loc, anyhow::Error>> {
    let t = match tokens.peek() {
        Tok::NameValue if matches!(tokens.content(), "address") => {
            tokens.advance()?;
            Type::Address
        }
        Tok::NameValue if matches!(tokens.content(), "u8") => {
            tokens.advance()?;
            Type::U8
        }
        Tok::NameValue if matches!(tokens.content(), "u16") => {
            tokens.advance()?;
            Type::U16
        }
        Tok::NameValue if matches!(tokens.content(), "u32") => {
            tokens.advance()?;
            Type::U32
        }
        Tok::NameValue if matches!(tokens.content(), "u64") => {
            tokens.advance()?;
            Type::U64
        }
        Tok::NameValue if matches!(tokens.content(), "u128") => {
            tokens.advance()?;
            Type::U128
        }
        Tok::NameValue if matches!(tokens.content(), "u256") => {
            tokens.advance()?;
            Type::U256
        }
        Tok::NameValue if matches!(tokens.content(), "bool") => {
            tokens.advance()?;
            Type::Bool
        }
        Tok::NameValue if matches!(tokens.content(), "signer") => {
            tokens.advance()?;
            Type::Signer
        }
        Tok::NameBeginTyValue if matches!(tokens.content(), "vector<") => {
            tokens.advance()?;
            let ty = parse_type(tokens)?;
            adjust_token(tokens, &[Tok::Greater])?;
            consume_token(tokens, Tok::Greater)?;
            Type::Vector(Box::new(ty))
        }
        Tok::DotNameValue => {
            let s = parse_qualified_struct_ident(tokens)?;
            let tys = parse_type_actuals(tokens)?;
            Type::Struct(s, tys)
        }
        Tok::Amp => {
            tokens.advance()?;
            Type::Reference(false, Box::new(parse_type(tokens)?))
        }
        Tok::AmpMut => {
            tokens.advance()?;
            Type::Reference(true, Box::new(parse_type(tokens)?))
        }
        Tok::NameValue => Type::TypeParameter(TypeVar_(parse_name(tokens)?)),
        t => {
            return Err(ParseError::InvalidToken {
                location: current_token_loc(tokens),
                message: format!("invalid token kind for type {:?}", t),
            })
        }
    };
    Ok(t)
}

// TypeVar: TypeVar = {
//     <n: Name> =>? TypeVar::parse(n),
// }
// TypeVar_ = Sp<TypeVar>;

fn parse_type_var(tokens: &mut Lexer) -> Result<TypeVar, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    let type_var = TypeVar_(parse_name(tokens)?);
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(tokens.file_hash(), start_loc, end_loc, type_var))
}

fn parse_type_parameter_with_phantom_decl(
    tokens: &mut Lexer,
) -> Result<StructTypeParameter, ParseError<Loc, anyhow::Error>> {
    let is_phantom = if tokens.peek() == Tok::NameValue && tokens.content() == "phantom" {
        tokens.advance()?;
        true
    } else {
        false
    };
    let (type_var, abilities) = parse_type_parameter(tokens)?;
    Ok((is_phantom, type_var, abilities))
}

// TypeFormal: (TypeVar_, Kind) = {
//     <type_var: Sp<TypeVar>> <k: (":" <Ability> ("+" <Ability>)*)?> =>? {
// }

fn parse_type_parameter(
    tokens: &mut Lexer,
) -> Result<(TypeVar, BTreeSet<Ability>), ParseError<Loc, anyhow::Error>> {
    let type_var = parse_type_var(tokens)?;
    if tokens.peek() == Tok::Colon {
        tokens.advance()?; // consume the ":"
        let abilities = parse_list(
            tokens,
            |tokens| {
                if tokens.peek() == Tok::Plus {
                    tokens.advance()?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
            parse_ability,
        )?;
        let mut ability_set = BTreeSet::new();
        for (ability, location) in abilities {
            let was_new_element = ability_set.insert(ability);
            if !was_new_element {
                return Err(ParseError::User {
                    location,
                    error: anyhow!("Duplicate ability '{}'", ability),
                });
            }
        }
        Ok((type_var, ability_set))
    } else {
        Ok((type_var, BTreeSet::new()))
    }
}

// TypeActuals: Vec<Type> = {
//     <tys: ('<' <Comma<Type>> ">")?> => { ... }
// }

fn parse_type_actuals(tokens: &mut Lexer) -> Result<Vec<Type>, ParseError<Loc, anyhow::Error>> {
    let tys = if tokens.peek() == Tok::Less {
        tokens.advance()?; // consume the '<'
        let list = parse_comma_list(tokens, &[Tok::Greater], parse_type, true)?;
        consume_token(tokens, Tok::Greater)?;
        list
    } else {
        vec![]
    };
    Ok(tys)
}

// NameAndTypeFormals: (String, Vec<(TypeVar_, Kind)>) = {
//     <n: NameBeginTy> <k: Comma<TypeFormal>> ">" => (n, k),
//     <n: Name> => (n, vec![]),
// }

fn parse_name_and_type_parameters<T, F>(
    tokens: &mut Lexer,
    param_parser: F,
) -> Result<(Symbol, Vec<T>), ParseError<Loc, anyhow::Error>>
where
    F: Fn(&mut Lexer) -> Result<T, ParseError<Loc, anyhow::Error>>,
{
    let mut has_types = false;
    let n = if tokens.peek() == Tok::NameBeginTyValue {
        has_types = true;
        parse_name_begin_ty(tokens)?
    } else {
        parse_name(tokens)?
    };
    let k = if has_types {
        let list = parse_comma_list(tokens, &[Tok::Greater], param_parser, true)?;
        consume_token(tokens, Tok::Greater)?;
        list
    } else {
        vec![]
    };
    Ok((n, k))
}

// NameAndTypeActuals: (String, Vec<Type>) = {
//     <n: NameBeginTy> '<' <tys: Comma<Type>> ">" => (n, tys),
//     <n: Name> => (n, vec![]),
// }

fn parse_name_and_type_actuals(
    tokens: &mut Lexer,
) -> Result<(Symbol, Vec<Type>), ParseError<Loc, anyhow::Error>> {
    let mut has_types = false;
    let n = if tokens.peek() == Tok::NameBeginTyValue {
        has_types = true;
        parse_name_begin_ty(tokens)?
    } else {
        parse_name(tokens)?
    };
    let tys = if has_types {
        let list = parse_comma_list(tokens, &[Tok::Greater], parse_type, true)?;
        consume_token(tokens, Tok::Greater)?;
        list
    } else {
        vec![]
    };
    Ok((n, tys))
}

// ArgDecl : (Var_, Type) = {
//     <v: Sp<Var>> ":" <t: Type> => (v, t)
// }

fn parse_arg_decl(tokens: &mut Lexer) -> Result<(Var, Type), ParseError<Loc, anyhow::Error>> {
    let v = parse_var(tokens)?;
    consume_token(tokens, Tok::Colon)?;
    let t = parse_type(tokens)?;
    Ok((v, t))
}

// ReturnType: Vec<Type> = {
//     ":" <t: Type> <v: ("*" <Type>)*> => { ... }
// }

fn parse_return_type(tokens: &mut Lexer) -> Result<Vec<Type>, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Colon)?;
    let t = parse_type(tokens)?;
    let mut v = vec![t];
    while tokens.peek() == Tok::Star {
        tokens.advance()?;
        v.push(parse_type(tokens)?);
    }
    Ok(v)
}

// AcquireList: Vec<StructName> = {
//     "acquires" <s: StructName> <al: ("," <StructName>)*> => { ... }
// }

fn parse_acquire_list(
    tokens: &mut Lexer,
) -> Result<Vec<StructName>, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Acquires)?;
    let s = parse_struct_name(tokens)?;
    let mut al = vec![s];
    while tokens.peek() == Tok::Comma {
        tokens.advance()?;
        al.push(parse_struct_name(tokens)?);
    }
    Ok(al)
}

//// Spec language parsing ////

// parses Name '.' Name and returns pair of strings.
fn spec_parse_dot_name(
    tokens: &mut Lexer,
) -> Result<(Symbol, Symbol), ParseError<Loc, anyhow::Error>> {
    let name1 = parse_name(tokens)?;
    consume_token(tokens, Tok::Period)?;
    let name2 = parse_name(tokens)?;
    Ok((name1, name2))
}

fn spec_parse_qualified_struct_ident(
    tokens: &mut Lexer,
) -> Result<QualifiedStructIdent, ParseError<Loc, anyhow::Error>> {
    let (m_string, n_string) = spec_parse_dot_name(tokens)?;
    let m: ModuleName = ModuleName(m_string);
    let n: StructName = StructName(n_string);
    Ok(QualifiedStructIdent::new(m, n))
}

fn parse_storage_location(
    tokens: &mut Lexer,
) -> Result<StorageLocation, ParseError<Loc, anyhow::Error>> {
    let base = match tokens.peek() {
        Tok::SpecReturn => {
            // RET(i)
            tokens.advance()?;
            let i = {
                if tokens.peek() == Tok::LParen {
                    consume_token(tokens, Tok::LParen)?;
                    let i = u8::from_str(tokens.content()).unwrap();
                    consume_token(tokens, Tok::U64Value)?;
                    consume_token(tokens, Tok::RParen)?;
                    i
                } else {
                    // RET without brackets; use RET(0)
                    0
                }
            };

            StorageLocation::Ret(i)
        }
        Tok::AccountAddressValue => StorageLocation::Address(parse_account_address(tokens)?),
        Tok::Global => {
            consume_token(tokens, Tok::Global)?;
            consume_token(tokens, Tok::Less)?;
            let type_ = spec_parse_qualified_struct_ident(tokens)?;
            let type_actuals = parse_type_actuals(tokens)?;
            consume_token(tokens, Tok::Greater)?;
            consume_token(tokens, Tok::LParen)?;
            let address = Box::new(parse_storage_location(tokens)?);
            consume_token(tokens, Tok::RParen)?;
            StorageLocation::GlobalResource {
                type_,
                type_actuals,
                address,
            }
        }
        _ => StorageLocation::Formal(parse_name(tokens)?),
    };

    // parsed the storage location base. now parse its fields and indices (if any)
    let mut fields_and_indices = vec![];
    loop {
        let tok = tokens.peek();
        if tok == Tok::Period {
            tokens.advance()?;
            fields_and_indices.push(FieldOrIndex::Field(parse_field(tokens)?.value));
        } else if tok == Tok::LSquare {
            tokens.advance()?;
            // Index expr can be ordinary expr, subrange, or update.
            let index_exp = parse_spec_exp(tokens)?;
            fields_and_indices.push(FieldOrIndex::Index(index_exp));
            consume_token(tokens, Tok::RSquare)?;
        } else {
            break;
        }
    }
    if fields_and_indices.is_empty() {
        Ok(base)
    } else {
        Ok(StorageLocation::AccessPath {
            base: Box::new(base),
            fields_and_indices,
        })
    }
}

fn parse_unary_spec_exp(tokens: &mut Lexer) -> Result<SpecExp, ParseError<Loc, anyhow::Error>> {
    Ok(match tokens.peek() {
        Tok::AccountAddressValue
        | Tok::True
        | Tok::False
        | Tok::U8Value
        | Tok::U16Value
        | Tok::U32Value
        | Tok::U64Value
        | Tok::U128Value
        | Tok::U256Value
        | Tok::ByteArrayValue => SpecExp::Constant(parse_copyable_val(tokens)?.value),
        Tok::GlobalExists => {
            consume_token(tokens, Tok::GlobalExists)?;
            consume_token(tokens, Tok::Less)?;
            let type_ = spec_parse_qualified_struct_ident(tokens)?;
            let type_actuals = parse_type_actuals(tokens)?;
            consume_token(tokens, Tok::Greater)?;
            consume_token(tokens, Tok::LParen)?;
            let address = parse_storage_location(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            SpecExp::GlobalExists {
                type_,
                type_actuals,
                address,
            }
        }
        Tok::Star => {
            tokens.advance()?;
            let stloc = parse_storage_location(tokens)?;
            SpecExp::Dereference(stloc)
        }
        Tok::Amp => {
            tokens.advance()?;
            let stloc = parse_storage_location(tokens)?;
            SpecExp::Reference(stloc)
        }
        Tok::Exclaim => {
            tokens.advance()?;
            let exp = parse_unary_spec_exp(tokens)?;
            SpecExp::Not(Box::new(exp))
        }
        Tok::Old => {
            tokens.advance()?;
            consume_token(tokens, Tok::LParen)?;
            let exp = parse_spec_exp(tokens)?;
            consume_token(tokens, Tok::RParen)?;
            SpecExp::Old(Box::new(exp))
        }
        Tok::NameValue => {
            let next = tokens.lookahead();
            if next.is_err() || next.unwrap() != Tok::LParen {
                SpecExp::StorageLocation(parse_storage_location(tokens)?)
            } else {
                let name = parse_name(tokens)?;
                let mut args = vec![];
                consume_token(tokens, Tok::LParen)?;
                while tokens.peek() != Tok::RParen {
                    let exp = parse_spec_exp(tokens)?;
                    args.push(exp);
                    if tokens.peek() != Tok::Comma {
                        break;
                    }
                    consume_token(tokens, Tok::Comma)?;
                }
                consume_token(tokens, Tok::RParen)?;
                SpecExp::Call(name, args)
            }
        }
        _ => SpecExp::StorageLocation(parse_storage_location(tokens)?),
    })
}

fn parse_rhs_of_spec_exp(
    tokens: &mut Lexer,
    lhs: SpecExp,
    min_prec: u32,
) -> Result<SpecExp, ParseError<Loc, anyhow::Error>> {
    let mut result = lhs;
    let mut next_tok_prec = get_precedence(tokens.peek());

    // Continue parsing binary expressions as long as they have they
    // specified minimum precedence.
    while next_tok_prec >= min_prec {
        let op_token = tokens.peek();
        tokens.advance()?;

        let mut rhs = parse_unary_spec_exp(tokens)?;

        // If the next token is another binary operator with a higher
        // precedence, then recursively parse that expression as the RHS.
        let this_prec = next_tok_prec;
        next_tok_prec = get_precedence(tokens.peek());
        if this_prec < next_tok_prec {
            rhs = parse_rhs_of_spec_exp(tokens, rhs, this_prec + 1)?;
            next_tok_prec = get_precedence(tokens.peek());
        }
        // TODO: Should we treat ==> like a normal BinOp?
        // TODO: Implement IFF
        if op_token == Tok::EqualEqualGreater {
            // Syntactic sugar: p ==> c ~~~> !p || c
            result = SpecExp::Binop(
                Box::new(SpecExp::Not(Box::new(result))),
                BinOp::Or,
                Box::new(rhs),
            );
        } else if op_token == Tok::ColonEqual {
            // it's an update expr
            result = SpecExp::Update(Box::new(result), Box::new(rhs))
        } else {
            let op = match op_token {
                Tok::EqualEqual => BinOp::Eq,
                Tok::ExclaimEqual => BinOp::Neq,
                Tok::Less => BinOp::Lt,
                Tok::Greater => BinOp::Gt,
                Tok::LessEqual => BinOp::Le,
                Tok::GreaterEqual => BinOp::Ge,
                Tok::PipePipe => BinOp::Or,
                Tok::AmpAmp => BinOp::And,
                Tok::Caret => BinOp::Xor,
                Tok::Pipe => BinOp::BitOr,
                Tok::Amp => BinOp::BitAnd,
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                Tok::PeriodPeriod => BinOp::Subrange,
                _ => panic!("Unexpected token that is not a binary operator"),
            };
            result = SpecExp::Binop(Box::new(result), op, Box::new(rhs))
        }
    }
    Ok(result)
}

fn parse_spec_exp(tokens: &mut Lexer) -> Result<SpecExp, ParseError<Loc, anyhow::Error>> {
    let lhs = parse_unary_spec_exp(tokens)?;
    parse_rhs_of_spec_exp(tokens, lhs, /* min_prec */ 1)
}

// Parse a top-level requires, modifies, ensures, aborts_if, or succeeds_if spec
// in a function decl.  This has to set the lexer into "spec_mode" to
// return names without eating trailing punctuation such as '<' or '.'.
// That is needed to parse paths with dots separating field names.
fn parse_spec_condition(tokens: &mut Lexer) -> Result<Condition_, ParseError<Loc, anyhow::Error>> {
    // Set lexer to read names without trailing punctuation
    tokens.spec_mode = true;
    let retval = Ok(match tokens.peek() {
        Tok::AbortsIf => {
            tokens.advance()?;
            Condition_::AbortsIf(parse_spec_exp(tokens)?)
        }
        Tok::Ensures => {
            tokens.advance()?;
            Condition_::Ensures(parse_spec_exp(tokens)?)
        }
        Tok::Requires => {
            tokens.advance()?;
            Condition_::Requires(parse_spec_exp(tokens)?)
        }
        Tok::SucceedsIf => {
            tokens.advance()?;
            Condition_::SucceedsIf(parse_spec_exp(tokens)?)
        }
        t => {
            tokens.spec_mode = false;
            return Err(ParseError::InvalidToken {
                location: current_token_loc(tokens),
                message: format!("invalid token kind for spec condition {:?}", t),
            });
        }
    });
    tokens.spec_mode = false;
    retval
}

fn parse_invariant(tokens: &mut Lexer) -> Result<Invariant, ParseError<Loc, anyhow::Error>> {
    // Set lexer to read names without trailing punctuation
    tokens.spec_mode = true;
    let start = tokens.start_loc();
    let result = parse_invariant_(tokens);
    tokens.spec_mode = false;
    Ok(spanned(
        tokens.file_hash(),
        start,
        tokens.previous_end_loc(),
        result?,
    ))
}

fn parse_invariant_(tokens: &mut Lexer) -> Result<Invariant_, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Invariant)?;
    let modifier = if tokens.peek() == Tok::LBrace {
        tokens.advance()?;
        let s = parse_name(tokens)?;
        consume_token(tokens, Tok::RBrace)?;
        Some(s)
    } else {
        None
    };
    // Check whether this invariant has the assignment form `invariant target = <expr>;`
    let target = if tokens.peek() == Tok::NameValue {
        // There must always be some token following (e.g. ;), so we can force lookahead.
        if tokens.lookahead()? == Tok::Equal {
            let name = parse_name(tokens)?;
            consume_token(tokens, Tok::Equal)?;
            Some(name)
        } else {
            None
        }
    } else {
        None
    };
    let condition = parse_spec_exp(tokens)?;
    Ok(Invariant_ {
        modifier,
        target,
        exp: condition,
    })
}

fn parse_synthetic(
    tokens: &mut Lexer,
) -> Result<SyntheticDefinition, ParseError<Loc, anyhow::Error>> {
    // Set lexer to read names without trailing punctuation
    tokens.spec_mode = true;
    let start = tokens.start_loc();
    let result = parse_synthetic_(tokens);
    tokens.spec_mode = false;
    Ok(spanned(
        tokens.file_hash(),
        start,
        tokens.previous_end_loc(),
        result?,
    ))
}

fn parse_synthetic_(
    tokens: &mut Lexer,
) -> Result<SyntheticDefinition_, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Synthetic)?;
    let field = parse_field(tokens)?;
    let name = field.value.0;
    consume_token(tokens, Tok::Colon)?;
    let type_ = parse_type(tokens)?;
    consume_token(tokens, Tok::Semicolon)?;
    Ok(SyntheticDefinition_ { name, type_ })
}

// FunctionVisibility : FunctionVisibility = {
//   (Public("("<v: Script | Friend>")")?)?
// }
fn parse_function_visibility(
    tokens: &mut Lexer,
) -> Result<FunctionVisibility, ParseError<Loc, anyhow::Error>> {
    let visibility = if match_token(tokens, Tok::Public)? {
        let sub_public_vis = if match_token(tokens, Tok::LParen)? {
            let sub_token = tokens.peek();
            match &sub_token {
                Tok::Script | Tok::Friend => (),
                t => {
                    return Err(ParseError::InvalidToken {
                        location: current_token_loc(tokens),
                        message: format!("expected Tok::Script or Tok::Friend, not {:?}", t),
                    });
                }
            }
            tokens.advance()?;
            consume_token(tokens, Tok::RParen)?;
            Some(sub_token)
        } else {
            None
        };
        match sub_public_vis {
            None => FunctionVisibility::Public,
            Some(Tok::Friend) => FunctionVisibility::Friend,
            _ => panic!("Unexpected token that is not a visibility modifier"),
        }
    } else {
        FunctionVisibility::Internal
    };
    Ok(visibility)
}

// FunctionDecl : (FunctionName, Function_) = {
//   <f: Sp<MoveFunctionDecl>> => (f.value.0, Spanned { span: f.loc, value: f.value.1 }),
//   <f: Sp<NativeFunctionDecl>> => (f.value.0, Spanned { span: f.loc, value: f.value.1 }),
// }

// MoveFunctionDecl : (FunctionName, Function) = {
//     <v: FunctionVisibility> <name_and_type_parameters: NameAndTypeFormals>
//     "(" <args: (ArgDecl)*> ")" <ret: ReturnType?>
//         <acquires: AcquireList?>
//         <locals_body: FunctionBlock> =>? { ... }
// }

// NativeFunctionDecl: (FunctionName, Function) = {
//     <nat: NativeTag> <v: FunctionVisibility> <name_and_type_parameters: NameAndTypeFormals>
//     "(" <args: Comma<ArgDecl>> ")" <ret: ReturnType?>
//         <acquires: AcquireList?>
//         ";" =>? { ... }
// }

fn parse_function_decl(
    tokens: &mut Lexer,
) -> Result<(FunctionName, Function), ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();

    let is_native = if tokens.peek() == Tok::Native {
        tokens.advance()?;
        true
    } else {
        false
    };

    let visibility = parse_function_visibility(tokens)?;
    let is_entry = if tokens.peek() == Tok::NameValue && tokens.content() == "entry" {
        tokens.advance()?;
        true
    } else {
        false
    };

    let (name, type_parameters) = parse_name_and_type_parameters(tokens, parse_type_parameter)?;
    consume_token(tokens, Tok::LParen)?;
    let args = parse_comma_list(tokens, &[Tok::RParen], parse_arg_decl, true)?;
    consume_token(tokens, Tok::RParen)?;

    let ret = if tokens.peek() == Tok::Colon {
        Some(parse_return_type(tokens)?)
    } else {
        None
    };

    let acquires = if tokens.peek() == Tok::Acquires {
        Some(parse_acquire_list(tokens)?)
    } else {
        None
    };

    // parse each specification directive--there may be zero or more
    let mut specifications = Vec::new();
    while tokens.peek().is_spec_directive() {
        let start_loc = tokens.start_loc();
        let cond = parse_spec_condition(tokens)?;
        let end_loc = tokens.previous_end_loc();
        specifications.push(spanned(tokens.file_hash(), start_loc, end_loc, cond));
    }

    let func_name = FunctionName(name);
    let func = Function_::new(
        visibility,
        is_entry,
        args,
        ret.unwrap_or_default(),
        type_parameters,
        acquires.unwrap_or_default(),
        specifications,
        if is_native {
            consume_token(tokens, Tok::Semicolon)?;
            FunctionBody::Native
        } else {
            let (locals, body) = parse_function_block_(tokens)?;
            FunctionBody::Move { locals, code: body }
        },
    );

    let end_loc = tokens.previous_end_loc();
    Ok((
        func_name,
        spanned(tokens.file_hash(), start_loc, end_loc, func),
    ))
}

// FieldDecl : (Field_, Type) = {
//     <f: Sp<Field>> ":" <t: Type> => (f, t)
// }

fn parse_field_decl(tokens: &mut Lexer) -> Result<(Field, Type), ParseError<Loc, anyhow::Error>> {
    let f = parse_field(tokens)?;
    consume_token(tokens, Tok::Colon)?;
    let t = parse_type(tokens)?;
    Ok((f, t))
}

// pub Script : Script = {
//     <imports: (ImportDecl)*>
//     "main" "(" <args: Comma<ArgDecl>> ")" <locals_body: FunctionBlock> => { ... }
// }

fn parse_script(tokens: &mut Lexer) -> Result<Script, ParseError<Loc, anyhow::Error>> {
    let script_start = tokens.start_loc();
    let mut imports: Vec<ImportDefinition> = vec![];
    while tokens.peek() == Tok::Import {
        imports.push(parse_import_decl(tokens)?);
    }
    let fun_start = tokens.start_loc();
    consume_token(tokens, Tok::Main)?;
    let type_formals = if tokens.peek() == Tok::Less {
        consume_token(tokens, Tok::Less)?;
        let list = parse_comma_list(tokens, &[Tok::Greater], parse_type_parameter, true)?;
        consume_token(tokens, Tok::Greater)?;
        list
    } else {
        vec![]
    };
    consume_token(tokens, Tok::LParen)?;
    let args = parse_comma_list(tokens, &[Tok::RParen], parse_arg_decl, true)?;
    consume_token(tokens, Tok::RParen)?;
    let (locals, code) = parse_function_block_(tokens)?;
    let end_loc = tokens.previous_end_loc();
    let main = Function_::new(
        FunctionVisibility::Public,
        /* is_entry */ true,
        args,
        vec![],
        type_formals,
        vec![],
        vec![],
        FunctionBody::Move { locals, code },
    );
    let main = spanned(tokens.file_hash(), fun_start, end_loc, main);
    let loc = make_loc(tokens.file_hash(), script_start, end_loc);
    Ok(Script::new(loc, imports, vec![], vec![], main))
}

// StructDecl: StructDefinition_ = {
//     "struct" <name_and_type_parameters:
//     NameAndTypeFormals> ("has" <Ability> ("," <Ability)*)? "{" <data: Comma<FieldDecl>> "}"
//     =>? { ... }
//     <native: NativeTag> <name_and_type_parameters: NameAndTypeFormals>
//     ("has" <Ability> ("," <Ability)*)?";" =>? { ... }
// }
fn parse_struct_decl(
    tokens: &mut Lexer,
) -> Result<StructDefinition, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();

    let is_native = if tokens.peek() == Tok::Native {
        tokens.advance()?;
        true
    } else {
        false
    };

    consume_token(tokens, Tok::Struct)?;
    let (name, type_parameters) =
        parse_name_and_type_parameters(tokens, parse_type_parameter_with_phantom_decl)?;

    let mut abilities = BTreeSet::new();
    if tokens.peek() == Tok::NameValue && tokens.content() == "has" {
        tokens.advance()?;
        let abilities_vec =
            parse_comma_list(tokens, &[Tok::LBrace, Tok::Semicolon], parse_ability, false)?;
        for (ability, location) in abilities_vec {
            let was_new_element = abilities.insert(ability);
            if !was_new_element {
                return Err(ParseError::User {
                    location,
                    error: anyhow!("Duplicate ability '{}'", ability),
                });
            }
        }
    }

    if is_native {
        consume_token(tokens, Tok::Semicolon)?;
        let end_loc = tokens.previous_end_loc();
        return Ok(spanned(
            tokens.file_hash(),
            start_loc,
            end_loc,
            StructDefinition_::native(abilities, name, type_parameters),
        ));
    }

    consume_token(tokens, Tok::LBrace)?;
    let fields = parse_comma_list(
        tokens,
        &[Tok::RBrace, Tok::Invariant],
        parse_field_decl,
        true,
    )?;
    let invariants = if tokens.peek() == Tok::Invariant {
        parse_comma_list(tokens, &[Tok::RBrace], parse_invariant, true)?
    } else {
        vec![]
    };
    consume_token(tokens, Tok::RBrace)?;
    let end_loc = tokens.previous_end_loc();
    Ok(spanned(
        tokens.file_hash(),
        start_loc,
        end_loc,
        StructDefinition_::move_declared(abilities, name, type_parameters, fields, invariants),
    ))
}

// ModuleIdent: ModuleIdent = {
//     <a: AccountAddress> "." <m: ModuleName> => ModuleIdent::new(m, a),
// }

fn parse_module_ident(tokens: &mut Lexer) -> Result<ModuleIdent, ParseError<Loc, anyhow::Error>> {
    let a = parse_account_address(tokens)?;
    consume_token(tokens, Tok::Period)?;
    let m = parse_module_name(tokens)?;
    Ok(ModuleIdent::new(m, a))
}

// FriendDecl: ModuleIdent = {
//     "friend" <ident: ModuleIdent> ";" => { ... }
// }

fn parse_friend_decl(tokens: &mut Lexer) -> Result<ModuleIdent, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Friend)?;
    let ident = parse_module_ident(tokens)?;
    consume_token(tokens, Tok::Semicolon)?;
    Ok(ident)
}

// ImportAlias: ModuleName = {
//     "as" <alias: ModuleName> => { ... }
// }

fn parse_import_alias(tokens: &mut Lexer) -> Result<ModuleName, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::As)?;
    let alias = parse_module_name(tokens)?;
    if alias == ModuleName::module_self() {
        panic!(
            "Invalid use of reserved module alias '{}'",
            ModuleName::self_name()
        );
    }
    Ok(alias)
}

// ImportDecl: ImportDefinition = {
//     "import" <ident: ModuleIdent> <alias: ImportAlias?> ";" => { ... }
// }

fn parse_import_decl(
    tokens: &mut Lexer,
) -> Result<ImportDefinition, ParseError<Loc, anyhow::Error>> {
    consume_token(tokens, Tok::Import)?;
    let ident = parse_module_ident(tokens)?;
    let alias = if tokens.peek() == Tok::As {
        Some(parse_import_alias(tokens)?)
    } else {
        None
    };
    consume_token(tokens, Tok::Semicolon)?;
    Ok(ImportDefinition::new(ident, alias))
}

// pub Module : ModuleDefinition = {
//     "module" <n: Name> "{"
//         <friends: (FriendDecl)*>
//         <imports: (ImportDecl)*>
//         <structs: (StructDecl)*>
//         <functions: (FunctionDecl)*>
//     "}" =>? ModuleDefinition::new(n, imports, structs, functions),
// }

fn is_struct_decl(tokens: &mut Lexer) -> Result<bool, ParseError<Loc, anyhow::Error>> {
    let t = tokens.peek();
    Ok(t == Tok::Struct || (t == Tok::Native && tokens.lookahead()? == Tok::Struct))
}

fn parse_module(tokens: &mut Lexer) -> Result<ModuleDefinition, ParseError<Loc, anyhow::Error>> {
    let start_loc = tokens.start_loc();
    consume_token(tokens, Tok::Module)?;
    let identifier = parse_module_ident(tokens)?;
    consume_token(tokens, Tok::LBrace)?;

    let mut friends = vec![];
    while tokens.peek() == Tok::Friend {
        friends.push(parse_friend_decl(tokens)?);
    }

    let mut imports = vec![];
    while tokens.peek() == Tok::Import {
        imports.push(parse_import_decl(tokens)?);
    }

    let mut synthetics = vec![];
    while tokens.peek() == Tok::Synthetic {
        synthetics.push(parse_synthetic(tokens)?);
    }

    let mut structs: Vec<StructDefinition> = vec![];
    while is_struct_decl(tokens)? {
        structs.push(parse_struct_decl(tokens)?);
    }

    let mut functions: Vec<(FunctionName, Function)> = vec![];
    while tokens.peek() != Tok::RBrace {
        functions.push(parse_function_decl(tokens)?);
    }
    tokens.advance()?; // consume the RBrace
    let end_loc = tokens.previous_end_loc();
    let loc = make_loc(tokens.file_hash(), start_loc, end_loc);

    Ok(ModuleDefinition::new(
        loc,
        identifier,
        friends,
        imports,
        vec![],
        structs,
        vec![],
        functions,
        synthetics,
    ))
}

// pub ScriptOrModule: ScriptOrModule = {
//     <s: Script> => ScriptOrModule::Script(s),
//     <m: Module> => ScriptOrModule::Module(m),
// }

fn parse_script_or_module(
    tokens: &mut Lexer,
) -> Result<ScriptOrModule, ParseError<Loc, anyhow::Error>> {
    if tokens.peek() == Tok::Module {
        Ok(ScriptOrModule::Module(parse_module(tokens)?))
    } else {
        Ok(ScriptOrModule::Script(parse_script(tokens)?))
    }
}

pub fn parse_module_string(
    input: &str,
) -> Result<ModuleDefinition, ParseError<Loc, anyhow::Error>> {
    let file_hash = FileHash::new(input);
    let mut tokens = Lexer::new(file_hash, input);
    tokens.advance()?;
    let unit = parse_module(&mut tokens)?;
    consume_token(&mut tokens, Tok::EOF)?;
    Ok(unit)
}

pub fn parse_script_string(input: &str) -> Result<Script, ParseError<Loc, anyhow::Error>> {
    let file_hash = FileHash::new(input);
    let mut tokens = Lexer::new(file_hash, input);
    tokens.advance()?;
    let unit = parse_script(&mut tokens)?;
    consume_token(&mut tokens, Tok::EOF)?;
    Ok(unit)
}

pub fn parse_script_or_module_string(
    input: &str,
) -> Result<ScriptOrModule, ParseError<Loc, anyhow::Error>> {
    let file_hash = FileHash::new(input);
    let mut tokens = Lexer::new(file_hash, input);
    tokens.advance()?;
    let unit = parse_script_or_module(&mut tokens)?;
    consume_token(&mut tokens, Tok::EOF)?;
    Ok(unit)
}
