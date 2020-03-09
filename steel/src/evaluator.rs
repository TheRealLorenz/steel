use std::cell::RefCell;
use std::convert::TryFrom;
use std::iter::Iterator;
use std::rc::Rc;
use std::result;

use crate::env::Env;
use crate::parser::tokens::Token;
use crate::parser::{Expr, ParseError, Parser};
use crate::rerrs::SteelErr;
use crate::rvals::{SteelLambda, SteelVal};
use crate::stop;
use std::collections::HashMap;
use std::ops::Deref;

pub type Result<T> = result::Result<T, SteelErr>;
pub type ValidFunc = fn(Vec<SteelVal>) -> Result<SteelVal>;

pub struct Evaluator {
    global_env: Rc<RefCell<Env>>,
    intern_cache: HashMap<String, Rc<Expr>>,
}

impl Evaluator {
    pub fn new() -> Self {
        Evaluator {
            global_env: Rc::new(RefCell::new(Env::default_env())),
            intern_cache: HashMap::new(),
        }
    }
    pub fn eval(&mut self, expr: Expr) -> Result<SteelVal> {
        // global environment updates automatically
        let expr = Rc::new(expr);
        evaluate(&expr, &self.global_env)
    }

    pub fn parse_and_eval(&mut self, expr_str: &str) -> Result<Vec<SteelVal>> {
        println!(
            "{:?}",
            self.intern_cache
                .iter()
                .map(|(x, y)| (x, Rc::strong_count(y)))
                .collect::<Vec<(&String, usize)>>()
        );
        let parsed: result::Result<Vec<Expr>, ParseError> =
            Parser::new(expr_str, &mut self.intern_cache).collect();
        let parsed = parsed?;
        parsed.into_iter().map(|x| self.eval(x)).collect()
    }

    pub fn clear_bindings(&mut self) {
        self.global_env.borrow_mut().clear_bindings();
    }

    pub fn insert_binding(&mut self, name: String, value: SteelVal) {
        self.global_env.borrow_mut().define(name, value);
    }

    pub fn insert_bindings(&mut self, vals: Vec<(&'static str, SteelVal)>) {
        self.global_env.borrow_mut().define_zipped(vals.into_iter());
    }

    pub fn lookup_binding(&mut self, name: &str) -> Result<SteelVal> {
        self.global_env.borrow_mut().lookup(name)
    }
}

impl Drop for Evaluator {
    fn drop(&mut self) {
        self.global_env.borrow_mut().clear_bindings();
        // println!(
        //     "{:?}",
        //     self.intern_cache
        //         .iter()
        //         .map(|(x, y)| (x, Rc::strong_count(y)))
        //         .collect::<Vec<(&String, usize)>>()
        // );
        self.intern_cache.clear();
        // println!(
        //     "{:?}",
        //     self.intern_cache
        //         .iter()
        //         .map(|(x, y)| (x, Rc::strong_count(y)))
        //         .collect::<Vec<(&String, usize)>>()
        // );
    }
}

fn parse_list_of_identifiers(identifiers: Rc<Expr>) -> Result<Vec<String>> {
    match identifiers.deref() {
        Expr::ListVal(l) => {
            let res: Result<Vec<String>> = l
                .iter()
                .map(|x| match &**x {
                    Expr::Atom(Token::Identifier(s)) => Ok(s.clone()),
                    _ => Err(SteelErr::TypeMismatch(
                        "Lambda must have symbols as arguments".to_string(),
                    )),
                })
                .collect();
            res
        }
        _ => Err(SteelErr::TypeMismatch("List of Identifiers".to_string())),
    }
}

/// returns error if tokens.len() != expected
fn check_length(what: &str, tokens: &[Rc<Expr>], expected: usize) -> Result<()> {
    if tokens.len() == expected {
        Ok(())
    } else {
        Err(SteelErr::ArityMismatch(format!(
            "{}: expected {} args got {}",
            what,
            expected,
            tokens.len()
        )))
    }
}

fn evaluate(expr: &Rc<Expr>, env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    let mut env = Rc::clone(env);
    let mut expr = Rc::clone(expr);

    loop {
        match expr.deref() {
            Expr::Atom(t) => return eval_atom(t, &env),

            Expr::ListVal(list_of_tokens) => {
                if let Some(f) = list_of_tokens.first() {
                    match f.deref() {
                        Expr::Atom(Token::Identifier(s)) if s == "quote" => {
                            check_length("Quote", &list_of_tokens, 2)?;
                            let converted = SteelVal::try_from(list_of_tokens[1].clone())?;
                            return Ok(converted);
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "if" => {
                            expr = eval_if(&list_of_tokens[1..], &env)?
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "define" => {
                            return eval_define(&list_of_tokens[1..], env).map(|_| SteelVal::Void)
                        }
                        // (lambda (vars*) (body))
                        Expr::Atom(Token::Identifier(s)) if s == "lambda" || s == "λ" => {
                            return eval_make_lambda(&list_of_tokens[1..], env);
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "eval" => {
                            return eval_eval_expr(&list_of_tokens[1..], &env)
                        }
                        // set! expression
                        Expr::Atom(Token::Identifier(s)) if s == "set!" => {
                            return eval_set(&list_of_tokens[1..], &env)
                        }
                        // (let (var binding)* (body))
                        Expr::Atom(Token::Identifier(s)) if s == "let" => {
                            expr = eval_let(&list_of_tokens[1..], &env)?
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "begin" => {
                            expr = eval_begin(&list_of_tokens[1..], &env)?
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "and" => {
                            return eval_and(&list_of_tokens[1..], &env)
                        }
                        Expr::Atom(Token::Identifier(s)) if s == "or" => {
                            return eval_or(&list_of_tokens[1..], &env)
                        }
                        // (sym args*), sym must be a procedure
                        _sym => match evaluate(f, &env)? {
                            SteelVal::FuncV(func) => {
                                return eval_func(func, &list_of_tokens[1..], &env)
                            }
                            SteelVal::LambdaV(lambda) => {
                                let (new_expr, new_env) =
                                    eval_lambda(lambda, &list_of_tokens[1..], &env)?;
                                expr = new_expr;
                                env = new_env;
                            }
                            e => stop!(TypeMismatch => e),
                        },
                    }
                } else {
                    stop!(TypeMismatch => "Given empty list")
                }
            }
        }
    }
}
/// evaluates an atom expression in given environment
fn eval_atom(t: &Token, env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    match t {
        Token::BooleanLiteral(b) => Ok(SteelVal::BoolV(*b)),
        Token::Identifier(s) => env.borrow().lookup(&s),
        Token::NumberLiteral(n) => Ok(SteelVal::NumV(*n)),
        Token::StringLiteral(s) => Ok(SteelVal::StringV(s.clone())),
        what => stop!(UnexpectedToken => what),
    }
}
/// evaluates a primitive function into single returnable value
fn eval_func(
    func: ValidFunc,
    list_of_tokens: &[Rc<Expr>],
    env: &Rc<RefCell<Env>>,
) -> Result<SteelVal> {
    let args_eval: Result<Vec<SteelVal>> =
        list_of_tokens.iter().map(|x| evaluate(&x, &env)).collect();
    let args_eval = args_eval?;
    // pure function doesn't need the env
    func(args_eval)
}

fn eval_and(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    for expr in list_of_tokens {
        match evaluate(expr, env)? {
            SteelVal::BoolV(true) => continue,
            SteelVal::BoolV(false) => return Ok(SteelVal::BoolV(false)),
            _ => continue,
        }
    }
    Ok(SteelVal::BoolV(true))
}

fn eval_or(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    for expr in list_of_tokens {
        match evaluate(expr, env)? {
            SteelVal::BoolV(true) => return Ok(SteelVal::BoolV(true)),
            _ => continue,
        }
    }
    Ok(SteelVal::BoolV(false))
}

/// evaluates a lambda into a body expression to execute
/// and an inner environment
fn eval_lambda(
    lambda: SteelLambda,
    list_of_tokens: &[Rc<Expr>],
    env: &Rc<RefCell<Env>>,
) -> Result<(Rc<Expr>, Rc<RefCell<Env>>)> {
    let args_eval: Result<Vec<SteelVal>> =
        list_of_tokens.iter().map(|x| evaluate(&x, &env)).collect();
    let args_eval: Vec<SteelVal> = args_eval?;
    // build a new environment using the parent environment
    let parent_env = lambda.parent_env();
    let inner_env = Rc::new(RefCell::new(Env::new(&parent_env)));
    let params_exp = lambda.params_exp();
    inner_env.borrow_mut().define_all(params_exp, args_eval)?;
    // loop back and continue
    // using the body as continuation
    // environment also gets updated
    Ok((lambda.body_exp(), inner_env))
}
/// evaluates `(test then else)` into `then` or `else`
fn eval_if(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<Rc<Expr>> {
    if let [test_expr, then_expr, else_expr] = list_of_tokens {
        match evaluate(&test_expr, env)? {
            SteelVal::BoolV(true) => Ok(then_expr.clone()),
            _ => Ok(else_expr.clone()),
        }
    } else {
        let e = format!("{}: expected {} args got {}", "If", 3, list_of_tokens.len());
        stop!(ArityMismatch => e);
    }
}

fn eval_make_lambda(list_of_tokens: &[Rc<Expr>], parent_env: Rc<RefCell<Env>>) -> Result<SteelVal> {
    if let [list_of_symbols, body_exp] = list_of_tokens {
        let parsed_list = parse_list_of_identifiers(list_of_symbols.clone())?;
        let constructed_lambda = SteelLambda::new(parsed_list, body_exp.clone(), parent_env);
        Ok(SteelVal::LambdaV(constructed_lambda))
    } else {
        let e = format!(
            "{}: expected {} args got {}",
            "Lambda",
            2,
            list_of_tokens.len()
        );
        stop!(ArityMismatch => e)
    }
}

// Evaluate all but the last, pass the last back up to the loop
fn eval_begin(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<Rc<Expr>> {
    let mut tokens_iter = list_of_tokens.iter();
    let last_token = tokens_iter.next_back();
    // throw away intermediate evaluations
    for token in tokens_iter {
        evaluate(token, env)?;
    }
    if let Some(v) = last_token {
        Ok(v.clone())
    } else {
        stop!(ArityMismatch => "begin requires at least one argument");
    }
}

fn eval_set(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    if let [symbol, rest_expr] = list_of_tokens {
        let value = evaluate(rest_expr, env)?;

        if let Expr::Atom(Token::Identifier(s)) = &**symbol {
            env.borrow_mut().set(s.clone(), value)
        } else {
            stop!(TypeMismatch => symbol)
        }
    } else {
        let e = format!(
            "{}: expected {} args got {}",
            "Set",
            2,
            list_of_tokens.len()
        );
        stop!(ArityMismatch => e)
    }
}

// TODO write tests
// Evaluate the inner expression, check that it is a quoted expression,
// evaluate body of quoted expression
fn eval_eval_expr(list_of_tokens: &[Rc<Expr>], env: &Rc<RefCell<Env>>) -> Result<SteelVal> {
    if let [e] = list_of_tokens {
        let res_expr = evaluate(e, env)?;
        match <Rc<Expr>>::try_from(res_expr) {
            Ok(e) => evaluate(&e, env),
            Err(_) => stop!(ContractViolation => "Eval not given an expression"),
        }
    } else {
        let e = format!(
            "{}: expected {} args got {}",
            "Eval",
            1,
            list_of_tokens.len()
        );
        stop!(ArityMismatch => e)
    }
}

// TODO maybe have to evaluate the params but i'm not sure
fn eval_define(list_of_tokens: &[Rc<Expr>], env: Rc<RefCell<Env>>) -> Result<Rc<RefCell<Env>>> {
    if let [symbol, body] = list_of_tokens {
        match symbol.deref() {
            Expr::Atom(Token::Identifier(s)) => {
                let eval_body = evaluate(body, &env)?;
                env.borrow_mut().define(s.to_string(), eval_body);
                Ok(env)
            }
            // construct lambda to parse
            Expr::ListVal(list_of_identifiers) => {
                if list_of_identifiers.is_empty() {
                    stop!(TypeMismatch => "define expected an identifier, got empty list")
                }
                if let Expr::Atom(Token::Identifier(s)) = &**&list_of_identifiers[0] {
                    // eval_make_lambda
                    let fake_lambda: Vec<Rc<Expr>> = vec![
                        Rc::new(Expr::Atom(Token::Identifier("lambda".to_string()))),
                        Rc::new(Expr::ListVal(list_of_identifiers[1..].to_vec())),
                        body.clone(),
                    ];

                    let constructed_lambda = Rc::new(Expr::ListVal(fake_lambda));

                    let eval_body = evaluate(&constructed_lambda, &env)?;
                    env.borrow_mut().define(s.to_string(), eval_body);
                    Ok(env)
                } else {
                    stop!(TypeMismatch => "Define expected identifier, got: {}", symbol);
                }
            }
            _ => stop!(TypeMismatch => "Define expects an identifier, got: {}", symbol),
        }
    } else {
        let e = format!(
            "{}: expected {} args got {}",
            "Define",
            2,
            list_of_tokens.len()
        );
        stop!(ArityMismatch => e)
    }
}

// Let is actually just a lambda so update values to be that and loop
// Syntax of a let -> (let ((a 10) (b 20) (c 25)) (body ...))
// transformed ((lambda (a b c) (body ...)) 10 20 25)
fn eval_let(list_of_tokens: &[Rc<Expr>], _env: &Rc<RefCell<Env>>) -> Result<Rc<Expr>> {
    if let [bindings, body] = list_of_tokens {
        let mut bindings_to_check: Vec<Rc<Expr>> = Vec::new();
        let mut args_to_check: Vec<Rc<Expr>> = Vec::new();

        // TODO fix this noise
        match bindings.deref() {
            Expr::ListVal(list_of_pairs) => {
                for pair in list_of_pairs {
                    match pair.deref() {
                        Expr::ListVal(p) => match p.as_slice() {
                            [binding, expression] => {
                                bindings_to_check.push(binding.clone());
                                args_to_check.push(expression.clone());
                            }
                            _ => stop!(BadSyntax => "Let requires pairs for binding"),
                        },
                        _ => stop!(BadSyntax => "Let: Missing body"),
                    }
                }
            }
            _ => stop!(BadSyntax => "Let: Missing name or binding pairs"),
        }

        let mut combined = vec![Rc::new(Expr::ListVal(vec![
            Rc::new(Expr::Atom(Token::Identifier("lambda".to_string()))),
            Rc::new(Expr::ListVal(bindings_to_check)),
            body.clone(),
        ]))];
        combined.append(&mut args_to_check);

        let application = Expr::ListVal(combined);
        Ok(Rc::new(application))
    } else {
        let e = format!(
            "{}: expected {} args got {}",
            "Let",
            2,
            list_of_tokens.len()
        );
        stop!(ArityMismatch => e)
    }
}

impl Default for Evaluator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod length_test {
    use super::*;
    use crate::parser::tokens::Token::NumberLiteral;
    use crate::parser::Expr::Atom;

    #[test]
    fn length_test() {
        let tokens = vec![
            Rc::new(Atom(NumberLiteral(1.0))),
            Rc::new(Atom(NumberLiteral(2.0))),
        ];
        assert!(check_length("Test", &tokens, 2).is_ok());
    }

    #[test]
    fn mismatch_test() {
        let tokens = vec![
            Rc::new(Atom(NumberLiteral(1.0))),
            Rc::new(Atom(NumberLiteral(2.0))),
        ];
        assert!(check_length("Test", &tokens, 1).is_err());
    }
}

#[cfg(test)]
mod parse_identifiers_test {
    use super::*;
    use crate::parser::tokens::Token::{Identifier, NumberLiteral};
    use crate::parser::Expr::{Atom, ListVal};

    #[test]
    fn non_symbols_test() {
        let identifier = Rc::new(ListVal(vec![
            Rc::new(Atom(NumberLiteral(1.0))),
            Rc::new(Atom(NumberLiteral(2.0))),
        ]));

        let res = parse_list_of_identifiers(identifier);

        assert!(res.is_err());
    }

    #[test]
    fn symbols_test() {
        let identifier = Rc::new(ListVal(vec![
            Rc::new(Atom(Identifier("a".to_string()))),
            Rc::new(Atom(Identifier("b".to_string()))),
        ]));

        let res = parse_list_of_identifiers(identifier);

        assert_eq!(res.unwrap(), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn malformed_test() {
        let identifier = Rc::new(Atom(Identifier("a".to_string())));

        let res = parse_list_of_identifiers(identifier);

        assert!(res.is_err());
    }
}

#[cfg(test)]
mod eval_make_lambda_test {
    use super::*;
    use crate::parser::tokens::Token::Identifier;
    use crate::parser::Expr::{Atom, ListVal};

    #[test]
    fn not_enough_args_test() {
        let list = vec![Rc::new(Atom(Identifier("a".to_string())))];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_make_lambda(&list, default_env);
        assert!(res.is_err());
    }

    #[test]
    fn not_list_val_test() {
        let list = vec![
            Rc::new(Atom(Identifier("a".to_string()))),
            Rc::new(Atom(Identifier("b".to_string()))),
            Rc::new(Atom(Identifier("c".to_string()))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_make_lambda(&list[1..], default_env);
        assert!(res.is_err());
    }

    #[test]
    fn ok_test() {
        let list = vec![
            Rc::new(Atom(Identifier("a".to_string()))),
            Rc::new(ListVal(vec![Rc::new(Atom(Identifier("b".to_string())))])),
            Rc::new(Atom(Identifier("c".to_string()))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_make_lambda(&list[1..], default_env);
        assert!(res.is_ok());
    }
}

#[cfg(test)]
mod eval_if_test {
    use super::*;
    use crate::parser::tokens::Token::BooleanLiteral;
    use crate::parser::Expr::Atom;

    #[test]
    fn true_test() {
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        //        let list = vec![Atom(If), ListVal(vec![Atom(StringLiteral(">".to_string())), Atom(StringLiteral("5".to_string())), Atom(StringLiteral("4".to_string()))]), Atom(BooleanLiteral(true)), Atom(BooleanLiteral(false))];
        let list = vec![
            Rc::new(Atom(Token::Identifier("if".to_string()))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(false))),
        ];
        let res = eval_if(&list[1..], &default_env);
        assert_eq!(res.unwrap(), Rc::new(Atom(BooleanLiteral(true))));
    }

    #[test]
    fn false_test() {
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let list = vec![
            Rc::new(Atom(Token::Identifier("if".to_string()))),
            Rc::new(Atom(BooleanLiteral(false))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(false))),
        ];
        let res = eval_if(&list[1..], &default_env);
        assert_eq!(res.unwrap(), Rc::new(Atom(BooleanLiteral(false))));
    }

    #[test]
    fn wrong_length_test() {
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let list = vec![
            Rc::new(Atom(Token::Identifier("if".to_string()))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(false))),
        ];
        let res = eval_if(&list[1..], &default_env);
        assert!(res.is_err());
    }
}

#[cfg(test)]
mod eval_define_test {
    use super::*;
    use crate::parser::tokens::Token::{BooleanLiteral, Identifier, StringLiteral};
    use crate::parser::Expr::{Atom, ListVal};

    #[test]
    fn wrong_length_test() {
        let list = vec![Rc::new(Atom(Identifier("a".to_string())))];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_define(&list[1..], default_env);
        assert!(res.is_err());
    }

    #[test]
    fn no_identifier_test() {
        let list = vec![Rc::new(Atom(StringLiteral("a".to_string())))];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_define(&list[1..], default_env);
        assert!(res.is_err());
    }

    #[test]
    fn atom_test() {
        let list = vec![
            Rc::new(Atom(Identifier("define".to_string()))),
            Rc::new(Atom(Identifier("a".to_string()))),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_define(&list[1..], default_env);
        assert!(res.is_ok());
    }

    #[test]
    fn list_val_test() {
        let list = vec![
            Rc::new(Atom(Identifier("define".to_string()))),
            Rc::new(ListVal(vec![Rc::new(Atom(Identifier("a".to_string())))])),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_define(&list[1..], default_env);
        assert!(res.is_ok());
    }

    #[test]
    fn list_val_no_identifier_test() {
        let list = vec![
            Rc::new(Atom(Identifier("define".to_string()))),
            Rc::new(ListVal(vec![Rc::new(Atom(StringLiteral("a".to_string())))])),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_define(&list[1..], default_env);
        assert!(res.is_err());
    }
}

#[cfg(test)]
mod eval_let_test {
    use super::*;
    use crate::parser::tokens::Token::{BooleanLiteral, NumberLiteral, StringLiteral};
    use crate::parser::Expr::{Atom, ListVal};

    #[test]
    fn ok_test() {
        let list = vec![
            Rc::new(Atom(Token::Identifier("let".to_string()))),
            Rc::new(ListVal(vec![Rc::new(ListVal(vec![
                Rc::new(Atom(StringLiteral("a".to_string()))),
                Rc::new(Atom(NumberLiteral(10.0))),
            ]))])),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_let(&list[1..], &default_env);
        assert!(res.is_ok());
    }

    #[test]
    fn missing_body_test() {
        let list = vec![
            Rc::new(Atom(Token::Identifier("let".to_string()))),
            Rc::new(ListVal(vec![Rc::new(ListVal(vec![Rc::new(Atom(
                NumberLiteral(10.0),
            ))]))])),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_let(&list[1..], &default_env);
        assert!(res.is_err());
    }

    #[test]
    fn missing_pair_binding_test() {
        let list = vec![
            Rc::new(Atom(Token::Identifier("let".to_string()))),
            Rc::new(Atom(Token::Identifier("let".to_string()))),
            Rc::new(Atom(BooleanLiteral(true))),
        ];
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let res = eval_let(&list[1..], &default_env);
        assert!(res.is_err());
    }
}

#[cfg(test)]
mod eval_test {
    use super::*;
    use crate::parser::tokens::Token::{BooleanLiteral, Identifier, NumberLiteral, StringLiteral};
    use crate::parser::Expr::{Atom, ListVal};

    #[test]
    fn boolean_test() {
        let input = Rc::new(Atom(BooleanLiteral(true)));
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        assert!(evaluate(&input, &default_env).is_ok());
    }

    #[test]
    fn identifier_test() {
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        let input = Rc::new(Atom(Identifier("+".to_string())));
        assert!(evaluate(&input, &default_env).is_ok());
    }

    #[test]
    fn number_test() {
        let input = Rc::new(Atom(NumberLiteral(10.0)));
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        assert!(evaluate(&input, &default_env).is_ok());
    }

    #[test]
    fn string_test() {
        let input = Rc::new(Atom(StringLiteral("test".to_string())));
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        assert!(evaluate(&input, &default_env).is_ok());
    }

    #[test]
    fn what_test() {
        let input = Rc::new(Atom(Identifier("if".to_string())));
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        assert!(evaluate(&input, &default_env).is_err());
    }

    #[test]
    fn list_if_test() {
        let list = vec![
            Rc::new(Atom(Identifier("if".to_string()))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(true))),
            Rc::new(Atom(BooleanLiteral(false))),
        ];
        let input = Rc::new(ListVal(list));
        let default_env = Rc::new(RefCell::new(Env::default_env()));
        assert!(evaluate(&input, &default_env).is_ok());
    }
}
