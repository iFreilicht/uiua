use std::{borrow::Cow, cell::RefCell, collections::HashMap};

use crate::{
    check::instrs_signature,
    function::{Function, FunctionKind, Instr},
    primitive::Primitive,
    value::Value,
};

impl Function {
    pub fn inverse(&self) -> Option<Self> {
        if !matches!(self.kind, FunctionKind::Normal) {
            return None;
        }
        Some(Function::new(
            self.id.clone(),
            invert_instrs(&self.instrs)?,
            FunctionKind::Normal,
        ))
    }
    pub fn under(self) -> Option<(Self, Self)> {
        if let Some(f) = self.inverse() {
            Some((self, f))
        } else {
            let (befores, afters) = under_instrs(&self.instrs)?;
            Some((
                Function::new(self.id.clone(), befores, FunctionKind::Normal),
                Function::new(self.id.clone(), afters, FunctionKind::Normal),
            ))
        }
    }
}

pub(crate) fn invert_instrs(instrs: &[Instr]) -> Option<Vec<Instr>> {
    if instrs.is_empty() {
        return Some(Vec::new());
    }

    thread_local! {
        static INVERT_CACHE: RefCell<HashMap<Vec<Instr>, Option<Vec<Instr>>>> = RefCell::new(HashMap::new());
    }
    if let Some(inverted) = INVERT_CACHE.with(|cache| cache.borrow().get(instrs).cloned()) {
        return inverted;
    }

    // println!("invert {:?}", instrs);
    let mut inverted = Vec::new();
    let mut start = instrs.len() - 1;
    let mut end = instrs.len();
    loop {
        if let Some(mut inverted_fragment) = invert_instr_fragment(&instrs[start..end]) {
            inverted_fragment.append(&mut inverted);
            inverted = inverted_fragment;
            if start == 0 {
                break;
            }
            end = start;
            start = end - 1;
        } else if start == 0 {
            return None;
        } else {
            start -= 1;
        }
    }
    // println!("inverted {:?} to {:?}", instrs, inverted);
    INVERT_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(instrs.to_vec(), Some(inverted.clone()))
    });
    Some(inverted)
}

fn invert_instr_fragment(instrs: &[Instr]) -> Option<Vec<Instr>> {
    use Instr::*;
    use Primitive::*;
    match instrs {
        [Prim(prim, span)] => {
            return Some(match prim {
                Primitive::Sqrt => vec![Instr::push(2.0), Instr::Prim(Primitive::Pow, *span)],
                prim => vec![Instr::Prim(prim.inverse()?, *span)],
            })
        }
        [Push(val)] => {
            if let Some((prim, span)) = val.as_primitive() {
                return Some(vec![Instr::Prim(prim.inverse()?, span)]);
            }
        }
        _ => {}
    }

    let patterns: &[&dyn InvertPattern] = &[
        &(Val, ([Invert], [Primitive::Call])),
        &(Val, ([Rotate], [Neg, Rotate])),
        &(Val, IgnoreMany(Flip), ([Add], [Sub])),
        &(Val, ([Sub], [Add])),
        &(Val, IgnoreMany(Flip), ([Mul], [Div])),
        &(Val, ([Div], [Mul])),
        &invert_pow_pattern,
        &invert_log_pattern,
        &invert_repeat_pattern,
    ];

    for pattern in patterns {
        let mut input = instrs;
        if let Some(inverted) = pattern.invert_extract(&mut input) {
            if input.is_empty() {
                return Some(inverted);
            }
        }
    }

    None
}

type Under = (Vec<Instr>, Vec<Instr>);

fn under_instrs(instrs: &[Instr]) -> Option<Under> {
    if instrs.is_empty() {
        return Some((Vec::new(), Vec::new()));
    }

    thread_local! {
        static UNDER_CACHE: RefCell<HashMap<Vec<Instr>, Option<Under>>> = RefCell::new(HashMap::new());
    }
    if let Some(under) = UNDER_CACHE.with(|cache| cache.borrow().get(instrs).cloned()) {
        return under;
    }

    let mut befores = Vec::new();
    let mut afters = Vec::new();
    let mut start = 0;
    let mut end = instrs.len();
    loop {
        if let Some((before, mut after)) = under_instr_fragment(&instrs[start..end]) {
            after.append(&mut afters);
            afters = after;
            match before {
                Cow::Borrowed(before) => befores.extend_from_slice(before),
                Cow::Owned(before) => befores.extend(before),
            }
            if start == 0 {
                break;
            }
            end = start;
            start = 0;
        } else if start == 0 {
            return None;
        } else {
            start += 1;
        }
    }
    // println!("under {:?} to {:?} {:?}", instrs, befores, afters);
    let under = (befores, afters);
    UNDER_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .insert(instrs.to_vec(), Some(under.clone()))
    });
    Some(under)
}

fn under_instr_fragment(instrs: &[Instr]) -> Option<(Cow<[Instr]>, Vec<Instr>)> {
    use Primitive::*;
    if let Some(inverted) = invert_instr_fragment(instrs) {
        return Some((Cow::Borrowed(instrs), inverted));
    }

    let patterns: &[&dyn UnderPattern] = &[
        &(Val, ([Take], [Over, Over, Take], [Untake])),
        &([Take], [Over, Over, Take], [Untake]),
        &(Val, ([Drop], [Over, Over, Drop], [Undrop])),
        &([Drop], [Over, Over, Drop], [Undrop]),
        &(Val, ([Select], [Over, Over, Select], [Unselect])),
        &([Select], [Over, Over, Select], [Unselect]),
        &(Val, ([Pick], [Over, Over, Pick], [Unpick])),
        &([Pick], [Over, Over, Pick], [Unpick]),
        &([Rotate], [Flip, Over, Rotate], [Flip, Neg, Rotate]),
        &(
            [First],
            [Dup, First],
            [Flip.i(), 1.i(), Drop.i(), Flip.i(), Join.i()],
        ),
        &(
            [Last],
            [Dup, Last],
            [Flip.i(), (-1).i(), Drop.i(), Join.i()],
        ),
    ];

    for pattern in patterns {
        let mut input = instrs;
        if let Some((befores, afters)) = pattern.under_extract(&mut input) {
            if input.is_empty() {
                return Some((Cow::Owned(befores), afters));
            }
        }
    }

    None
}

trait AsInstr {
    fn as_instr(&self, span: usize) -> Instr;
    fn i(&self) -> Box<dyn AsInstr>
    where
        Self: Copy + 'static,
    {
        Box::new(*self)
    }
}

impl AsInstr for i32 {
    fn as_instr(&self, _: usize) -> Instr {
        Instr::push(Value::from(*self))
    }
}

impl AsInstr for Primitive {
    fn as_instr(&self, span: usize) -> Instr {
        Instr::Prim(*self, span)
    }
}

impl AsInstr for Box<dyn AsInstr> {
    fn as_instr(&self, span: usize) -> Instr {
        self.as_ref().as_instr(span)
    }
}

trait InvertPattern {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>>;
}

trait UnderPattern {
    fn under_extract(&self, input: &mut &[Instr]) -> Option<Under>;
}

impl<A: InvertPattern, B: InvertPattern> InvertPattern for (A, B) {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        let (a, b) = self;
        let mut a = a.invert_extract(input)?;
        let b = b.invert_extract(input)?;
        a.extend(b);
        Some(a)
    }
}

impl<A: UnderPattern, B: UnderPattern> UnderPattern for (A, B) {
    fn under_extract(&self, input: &mut &[Instr]) -> Option<Under> {
        let (a, b) = self;
        let (mut a_before, a_after) = a.under_extract(input)?;
        let (b_before, mut b_after) = b.under_extract(input)?;
        a_before.extend(b_before);
        b_after.extend(a_after);
        Some((a_before, b_after))
    }
}

impl<A: InvertPattern, B: InvertPattern, C: InvertPattern> InvertPattern for (A, B, C) {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        let (a, b, c) = self;
        let mut a = a.invert_extract(input)?;
        let b = b.invert_extract(input)?;
        let c = c.invert_extract(input)?;
        a.extend(b);
        a.extend(c);
        Some(a)
    }
}

struct IgnoreMany<T>(T);
impl<T: InvertPattern> InvertPattern for IgnoreMany<T> {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        while self.0.invert_extract(input).is_some() {}
        Some(Vec::new())
    }
}

struct AnyOf<T, const N: usize>([T; N]);
impl<T: InvertPattern, const N: usize> InvertPattern for AnyOf<T, N> {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        for pattern in self.0.iter() {
            let mut inp = *input;
            if let Some(inverted) = pattern.invert_extract(&mut inp) {
                *input = inp;
                return Some(inverted);
            }
        }
        None
    }
}

impl InvertPattern for Primitive {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        let next = input.get(0)?;
        match next {
            Instr::Prim(prim, span) if prim == self => {
                *input = &input[1..];
                Some(vec![Instr::Prim(*prim, *span)])
            }
            _ => None,
        }
    }
}

impl<T> InvertPattern for (&[Primitive], &[T])
where
    T: AsInstr,
{
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        let (a, b) = *self;
        if a.len() > input.len() {
            return None;
        }
        let mut spans = Vec::new();
        for (instr, prim) in input.iter().zip(a.iter()) {
            match instr {
                Instr::Prim(instr_prim, span) if instr_prim == prim => spans.push(*span),
                _ => return None,
            }
        }
        *input = &input[a.len()..];
        Some(
            b.iter()
                .zip(spans.iter().cycle())
                .map(|(p, s)| p.as_instr(*s))
                .collect(),
        )
    }
}

impl<A, B> UnderPattern for (&[Primitive], &[A], &[B])
where
    A: AsInstr,
    B: AsInstr,
{
    fn under_extract(&self, input: &mut &[Instr]) -> Option<Under> {
        let (a, b, c) = *self;
        if a.len() > input.len() {
            return None;
        }
        let mut spans = Vec::new();
        for (instr, prim) in input.iter().zip(a.iter()) {
            match instr {
                Instr::Prim(instr_prim, span) if instr_prim == prim => spans.push(*span),
                _ => return None,
            }
        }
        *input = &input[a.len()..];
        Some((
            b.iter()
                .zip(spans.iter().cycle())
                .map(|(p, s)| p.clone().as_instr(*s))
                .collect(),
            c.iter()
                .zip(spans.iter().cycle())
                .map(|(p, s)| p.clone().as_instr(*s))
                .collect(),
        ))
    }
}

impl<T, const A: usize, const B: usize> InvertPattern for ([Primitive; A], [T; B])
where
    T: AsInstr,
{
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        let (a, b) = self;
        (a.as_ref(), b.as_ref()).invert_extract(input)
    }
}

impl<T, U, const A: usize, const B: usize, const C: usize> UnderPattern
    for ([Primitive; A], [T; B], [U; C])
where
    T: AsInstr,
    U: AsInstr,
{
    fn under_extract(&self, input: &mut &[Instr]) -> Option<Under> {
        let (a, b, c) = self;
        (a.as_ref(), b.as_ref(), c.as_ref()).under_extract(input)
    }
}

impl<F> InvertPattern for F
where
    F: Fn(&mut &[Instr]) -> Option<Vec<Instr>>,
{
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        self(input)
    }
}

fn invert_pow_pattern(input: &mut &[Instr]) -> Option<Vec<Instr>> {
    let val = Val.invert_extract(input)?;
    let next = input.get(0)?;
    if let Instr::Prim(Primitive::Pow, span) = next {
        *input = &input[1..];
        Some(vec![
            Instr::push(1u8),
            val[0].clone(),
            Instr::Prim(Primitive::Div, *span),
            Instr::Prim(Primitive::Pow, *span),
        ])
    } else {
        None
    }
}

fn invert_log_pattern(input: &mut &[Instr]) -> Option<Vec<Instr>> {
    let val = Val.invert_extract(input)?;
    let next = input.get(0)?;
    if let Instr::Prim(Primitive::Log, span) = next {
        *input = &input[1..];
        Some(vec![
            val[0].clone(),
            Instr::Prim(Primitive::Flip, *span),
            Instr::Prim(Primitive::Pow, *span),
        ])
    } else {
        None
    }
}

fn invert_repeat_pattern(input: &mut &[Instr]) -> Option<Vec<Instr>> {
    let start = *input;
    let mut instrs = Val.invert_extract(input)?;
    if let [Instr::Push(f), Instr::Prim(Primitive::Repeat, span), ..] = input {
        instrs.push(Instr::Prim(Primitive::Neg, *span));
        instrs.push(Instr::Push(f.clone()));
        instrs.push(Instr::Prim(Primitive::Repeat, *span));
        *input = &input[2..];
        Some(instrs)
    } else {
        *input = start;
        None
    }
}

struct Val;
impl InvertPattern for Val {
    fn invert_extract(&self, input: &mut &[Instr]) -> Option<Vec<Instr>> {
        if input.is_empty() {
            return Some(Vec::new());
        }
        for len in (1..input.len()).rev() {
            let chunk = &input[..len];
            if let Ok(sig) = instrs_signature(chunk) {
                if sig.args == 0 && sig.outputs == 1 {
                    let res = chunk.to_vec();
                    *input = &input[len..];
                    return Some(res);
                }
            }
        }
        match input.get(0) {
            Some(instr @ Instr::Push(_)) => {
                *input = &input[1..];
                Some(vec![instr.clone()])
            }
            Some(instr @ Instr::Prim(prim, _))
                if prim.args() == Some(0) && prim.outputs() == Some(0) =>
            {
                *input = &input[1..];
                Some(vec![instr.clone()])
            }
            Some(Instr::BeginArray) => {
                let mut depth = 1;
                let mut i = 1;
                loop {
                    if let Instr::EndArray { .. } = input.get(i)? {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    } else if let Instr::BeginArray = input.get(i)? {
                        depth += 1;
                    }
                    i += 1;
                }
                let array_construction = &input[..=i];
                *input = &input[i + 1..];
                Some(array_construction.to_vec())
            }
            _ => None,
        }
    }
}
impl UnderPattern for Val {
    fn under_extract(&self, input: &mut &[Instr]) -> Option<Under> {
        self.invert_extract(input).map(|v| (v, Vec::new()))
    }
}
