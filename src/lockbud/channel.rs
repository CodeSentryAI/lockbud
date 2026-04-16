//! Channel deadlock detector for std::sync::mpsc.
//!
//! Detects patterns where a blocking send/recv on a channel can deadlock,
//! e.g. sync_channel(0) followed by send-then-recv in the same thread.

use charon_lib::ast::*;
use charon_lib::export::CrateData;
use charon_lib::ullbc_ast;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

use crate::lockbud::report::{ChannelDeadlockDiagnosis, Report, ReportContent};
use crate::lockbud::types::Location;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelApi {
    Channel,
    SyncChannel,
    Send,
    Recv,
}

fn format_name(name: &Name) -> String {
    name.name
        .iter()
        .map(|elem| match elem {
            PathElem::Ident(s, _) => s.clone(),
            PathElem::Impl(_) => "{impl}".to_string(),
            PathElem::Instantiated(_) => "{inst}".to_string(),
        })
        .collect::<Vec<_>>()
        .join("::")
}

pub fn classify_channel_api(func: &FnOperand, krate: &TranslatedCrate) -> Option<ChannelApi> {
    match func {
        FnOperand::Regular(fn_ptr) => match fn_ptr.kind.as_ref() {
            FnPtrKind::Fun(FunId::Regular(fun_id)) => {
                let decl = krate.fun_decls.get(*fun_id)?;
                let name = format_name(&decl.item_meta.name);
                if name.starts_with("std::sync::mpsc::sync_channel") {
                    Some(ChannelApi::SyncChannel)
                } else if name.starts_with("std::sync::mpsc::channel") {
                    Some(ChannelApi::Channel)
                } else if name.contains("SyncSender") && name.contains("::send") {
                    Some(ChannelApi::Send)
                } else if name.contains("Sender") && name.contains("::send") {
                    Some(ChannelApi::Send)
                } else if name.contains("Receiver") && name.contains("::recv") {
                    Some(ChannelApi::Recv)
                } else {
                    None
                }
            }
            _ => None,
        },
        FnOperand::Dynamic(_) => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChannelId(pub usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EndpointKind {
    Sender,
    Receiver,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EndpointId {
    pub channel: ChannelId,
    pub kind: EndpointKind,
}

#[derive(Clone, Debug)]
pub struct EndpointLifetime {
    pub creation_loc: (FunDeclId, Location),
    pub drop_locs: Vec<(FunDeclId, Location)>,
    pub sync_locs: Vec<(FunDeclId, Location)>,
    pub escapes: bool,
    pub moved_to_thread: bool,
}

/// A channel-related callsite.
#[derive(Clone, Debug)]
pub struct ChannelCallSite {
    pub api: ChannelApi,
    pub loc: Location,
    /// For constructors: the destination local of the returned tuple.
    pub tuple_dest: Option<LocalId>,
    /// For send/recv: the local of the channel object (sender/receiver).
    pub ch_local: Option<LocalId>,
    /// For send/recv: the tuple local of the constructor they belong to.
    pub tuple_base: Option<LocalId>,
    /// For send/recv: which field of the tuple (0 = Sender, 1 = Receiver).
    pub tuple_field_index: Option<usize>,
}

pub type ChannelCallSites = FxHashMap<FunDeclId, Vec<ChannelCallSite>>;

/// Collect channel API callsites per function.
pub fn collect_channel_callsites(crate_data: &CrateData) -> ChannelCallSites {
    let mut result = ChannelCallSites::default();
    let krate = &crate_data.translated;

    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        if !fun_decl.item_meta.is_local {
            continue;
        }

        // Track ref places for alias resolution.
        let ref_places = build_ref_places(body);
        // Track move/copy chains: dest = Use(Move(src)) / Use(Copy(src))
        let move_sources = build_move_sources(body);
        // Track tuple projections: dest = Projection(base, Field)
        let tuple_fields = build_tuple_fields(body);

        for (block_id, block) in body.body.iter_indexed() {
            let term_loc = Location::new(block_id, block.statements.len());
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let Some(api) = classify_channel_api(&call.func, krate) {
                    match api {
                        ChannelApi::Channel | ChannelApi::SyncChannel => {
                            let tuple_dest = call.dest.local_id();
                            result.entry(fun_id).or_default().push(ChannelCallSite {
                                api,
                                loc: term_loc,
                                tuple_dest,
                                ch_local: None,
                                tuple_base: None,
                                tuple_field_index: None,
                            });
                        }
                        ChannelApi::Send | ChannelApi::Recv => {
                            let ch_local = call
                                .args
                                .first()
                                .and_then(|arg| match arg {
                                    Operand::Copy(p) | Operand::Move(p) => {
                                        resolve_local(p, &ref_places, &move_sources)
                                    }
                                    Operand::Const(_) => None,
                                });
                            // Try to resolve the tuple constructor this send/recv belongs to.
                            let (tuple_base, field_index) = ch_local
                                .and_then(|l| tuple_fields.get(&l))
                                .map(|(base, idx)| {
                                    let has_ctor = result
                                        .get(&fun_id)
                                        .map(|sites| {
                                            sites.iter().any(|s| {
                                                matches!(s.api, ChannelApi::Channel | ChannelApi::SyncChannel)
                                                    && s.tuple_dest == Some(*base)
                                            })
                                        })
                                        .unwrap_or(false);
                                    if has_ctor {
                                        (Some(*base), Some(*idx))
                                    } else {
                                        (None, None)
                                    }
                                })
                                .unwrap_or((None, None));
                            result.entry(fun_id).or_default().push(ChannelCallSite {
                                api,
                                loc: term_loc,
                                tuple_dest: None,
                                ch_local,
                                tuple_base,
                                tuple_field_index: field_index,
                            });
                        }
                    }
                }
            }
        }
    }

    result
}

fn build_ref_places(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, Place> {
    let mut map = FxHashMap::default();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if let ullbc_ast::Rvalue::Ref { place, .. } = rvalue {
                        map.insert(dest_local, place.clone());
                    }
                }
            }
        }
    }
    map
}

fn build_move_sources(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, LocalId> {
    let mut map = FxHashMap::default();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if let ullbc_ast::Rvalue::Use(Operand::Move(p) | Operand::Copy(p)) = rvalue {
                        if let Some(src_local) = p.local_id() {
                            map.insert(dest_local, src_local);
                        }
                    }
                }
            }
        }
    }
    map
}

/// local -> (base_tuple_local, field_index)
fn build_tuple_fields(body: &ullbc_ast::ExprBody) -> FxHashMap<LocalId, (LocalId, usize)> {
    let mut map = FxHashMap::default();
    for block in &body.body {
        for stmt in &block.statements {
            if let ullbc_ast::StatementKind::Assign(dest, rvalue) = &stmt.kind {
                if let Some(dest_local) = dest.local_id() {
                    if let ullbc_ast::Rvalue::Use(Operand::Move(p) | Operand::Copy(p)) = rvalue {
                        if let PlaceKind::Projection(base, elem) = &p.kind {
                            if let ProjectionElem::Field(_, field_id) = elem {
                                if let Some(base_local) = base.local_id() {
                                    map.insert(dest_local, (base_local, field_id.index()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    map
}

fn resolve_local(
    place: &Place,
    ref_places: &FxHashMap<LocalId, Place>,
    move_sources: &FxHashMap<LocalId, LocalId>,
) -> Option<LocalId> {
    let local = place.local_id()?;
    // First try move source.
    if let Some(src) = move_sources.get(&local) {
        return Some(*src);
    }
    // Then try ref place.
    if let Some(underlying) = ref_places.get(&local) {
        return underlying.local_id();
    }
    Some(local)
}

/// Detect channel deadlocks intra-procedurally.
/// Current strategy:
/// - For sync_channel constructors, find paired send/recv in the same function.
/// - If a `send` is reachable before a `recv` on the same channel in the same thread,
///   report a potential deadlock (send blocks waiting for recv, but recv is after).
pub fn detect_channel_deadlocks(
    crate_data: &CrateData,
    channel_callsites: &ChannelCallSites,
) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;

    for (fun_id, sites) in channel_callsites {
        let Some(fun_decl) = krate.fun_decls.get(*fun_id) else {
            continue;
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };

        // Identify sync_channel constructors in this function.
        let constructors: Vec<_> = sites
            .iter()
            .filter(|s| matches!(s.api, ChannelApi::SyncChannel))
            .collect();

        if constructors.is_empty() {
            continue;
        }

        // Build a map from tuple local -> constructor site.
        let mut ctor_by_tuple: FxHashMap<LocalId, &ChannelCallSite> = FxHashMap::default();
        for ctor in &constructors {
            if let Some(t) = ctor.tuple_dest {
                ctor_by_tuple.insert(t, *ctor);
            }
        }

        // Map: channel tuple local -> (sender locals, receiver locals) extracted from tuple.
        let mut senders: FxHashMap<LocalId, FxHashSet<LocalId>> = FxHashMap::default();
        let mut receivers: FxHashMap<LocalId, FxHashSet<LocalId>> = FxHashMap::default();

        for site in sites {
            if let Some(tuple_base) = site.tuple_base {
                if matches!(site.api, ChannelApi::Send) {
                    senders.entry(tuple_base).or_default().insert(site.ch_local.unwrap_or(tuple_base));
                } else if matches!(site.api, ChannelApi::Recv) {
                    receivers.entry(tuple_base).or_default().insert(site.ch_local.unwrap_or(tuple_base));
                }
            }
        }

        // For each sync_channel constructor, check send/recv deadlock.
        for (tuple_local, _) in &ctor_by_tuple {
            let sends = senders.get(tuple_local).cloned().unwrap_or_default();
            let recvs = receivers.get(tuple_local).cloned().unwrap_or_default();

            for send_local in &sends {
                for recv_local in &recvs {
                    // Find actual callsites for this send/recv local.
                    let send_sites: Vec<_> = sites
                        .iter()
                        .filter(|s| s.api == ChannelApi::Send && s.ch_local == Some(*send_local) && s.tuple_base == Some(*tuple_local))
                        .collect();
                    let recv_sites: Vec<_> = sites
                        .iter()
                        .filter(|s| s.api == ChannelApi::Recv && s.ch_local == Some(*recv_local) && s.tuple_base == Some(*tuple_local))
                        .collect();

                    for send_site in &send_sites {
                        for recv_site in &recv_sites {
                            // Deadlock pattern: send is reachable before recv in the same function.
                            if is_reachable(send_site.loc, recv_site.loc, body) {
                                let fn_name = format_name(&fun_decl.item_meta.name);
                                let send_span = span_for_loc(body, send_site.loc);
                                let recv_span = span_for_loc(body, recv_site.loc);
                                let diagnosis = ChannelDeadlockDiagnosis {
                                    fn_name,
                                    channel_type: "std::sync::mpsc::sync_channel".to_owned(),
                                    send_span: format!("{:?}", send_span),
                                    recv_span: format!("{:?}", recv_span),
                                    explanation:
                                        "Blocking send on sync_channel is reachable before recv in the same thread"
                                            .to_owned(),
                                };
                                reports.push(Report::ChannelDeadlock(ReportContent::new(
                                    "ChannelDeadlock".to_owned(),
                                    "Possibly".to_owned(),
                                    diagnosis,
                                    "Blocking send on sync_channel is reachable before recv in the same thread"
                                        .to_owned(),
                                )));
                            }
                        }
                    }
                }
            }
        }
    }

    reports
}

fn span_for_loc(body: &ullbc_ast::ExprBody, loc: Location) -> Span {
    let block = &body.body[loc.block];
    if loc.statement_index < block.statements.len() {
        block.statements[loc.statement_index].span
    } else {
        block.terminator.span
    }
}

fn is_reachable(from: Location, to: Location, body: &ullbc_ast::ExprBody) -> bool {
    if from.block == to.block {
        return from.statement_index <= to.statement_index;
    }
    let mut visited = FxHashSet::default();
    let mut queue = VecDeque::new();
    queue.push_back(from.block);
    visited.insert(from.block);
    while let Some(curr) = queue.pop_front() {
        if curr == to.block {
            return true;
        }
        let block = &body.body[curr];
        for succ in terminator_successors(&block.terminator.kind) {
            if visited.insert(*succ) {
                queue.push_back(*succ);
            }
        }
    }
    false
}

fn terminator_successors(term: &ullbc_ast::TerminatorKind) -> Vec<&ullbc_ast::BlockId> {
    use ullbc_ast::TerminatorKind::*;
    match term {
        Goto { target } => vec![target],
        Switch { targets, .. } => match targets {
            ullbc_ast::SwitchTargets::If(t1, t2) => vec![t1, t2],
            ullbc_ast::SwitchTargets::SwitchInt(_, vals, default) => {
                let mut v: Vec<_> = vals.iter().map(|(_, b)| b).collect();
                v.push(default);
                v
            }
        },
        Call { target, on_unwind, .. } => vec![target, on_unwind],
        Drop { target, on_unwind, .. } => vec![target, on_unwind],
        Assert { target, on_unwind, .. } => vec![target, on_unwind],
        Return | Abort(_) | UnwindResume => vec![],
    }
}

/// Build a map from (fun_id, local_id) to endpoint_id, channel registry,
/// and endpoint lifetime summary (intra-procedural only).
pub fn build_endpoint_map(
    crate_data: &CrateData,
    callsites: &ChannelCallSites,
) -> (
    FxHashMap<(FunDeclId, LocalId), EndpointId>,
    FxHashMap<ChannelId, (EndpointId, EndpointId)>,
    FxHashMap<EndpointId, EndpointLifetime>,
) {
    let krate = &crate_data.translated;
    let mut channels = FxHashMap::default();
    let mut tuple_to_channel: FxHashMap<(FunDeclId, LocalId), ChannelId> = FxHashMap::default();
    let mut local_to_endpoint: FxHashMap<(FunDeclId, LocalId), EndpointId> = FxHashMap::default();
    let mut lifetimes: FxHashMap<EndpointId, EndpointLifetime> = FxHashMap::default();
    let mut next_channel_id = 0usize;

    for (fun_id, sites) in callsites {
        // First pass: register constructors.
        for site in sites {
            if matches!(site.api, ChannelApi::Channel | ChannelApi::SyncChannel) {
                if let Some(tuple_local) = site.tuple_dest {
                    let cid = ChannelId(next_channel_id);
                    next_channel_id += 1;
                    let sender = EndpointId {
                        channel: cid,
                        kind: EndpointKind::Sender,
                    };
                    let receiver = EndpointId {
                        channel: cid,
                        kind: EndpointKind::Receiver,
                    };
                    channels.insert(cid, (sender, receiver));
                    tuple_to_channel.insert((*fun_id, tuple_local), cid);

                    lifetimes.insert(
                        sender,
                        EndpointLifetime {
                            creation_loc: (*fun_id, site.loc),
                            drop_locs: Vec::new(),
                            sync_locs: Vec::new(),
                            escapes: false,
                            moved_to_thread: false,
                        },
                    );
                    lifetimes.insert(
                        receiver,
                        EndpointLifetime {
                            creation_loc: (*fun_id, site.loc),
                            drop_locs: Vec::new(),
                            sync_locs: Vec::new(),
                            escapes: false,
                            moved_to_thread: false,
                        },
                    );
                }
            }
        }

        // Second pass: map send/recv locals to endpoints and record sync usage.
        for site in sites {
            if let (Some(ch_local), Some(tuple_base), Some(field_idx)) =
                (site.ch_local, site.tuple_base, site.tuple_field_index)
            {
                if let Some(&cid) = tuple_to_channel.get(&(*fun_id, tuple_base)) {
                    if let Some(&(sender, receiver)) = channels.get(&cid) {
                        let endpoint = if field_idx == 0 { sender } else { receiver };
                        local_to_endpoint.insert((*fun_id, ch_local), endpoint);
                        if let Some(lt) = lifetimes.get_mut(&endpoint) {
                            lt.sync_locs.push((*fun_id, site.loc));
                        }
                    }
                }
            }
        }
    }

    // Third pass: scan for thread::spawn moves of endpoints.
    for (fun_id, fun_decl) in krate.fun_decls.iter_indexed() {
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };
        for block in &body.body {
            if let ullbc_ast::TerminatorKind::Call { call, .. } = &block.terminator.kind {
                if let FnOperand::Regular(fn_ptr) = &call.func {
                    if let FnPtrKind::Fun(FunId::Regular(callee_id)) = fn_ptr.kind.as_ref() {
                        if let Some(decl) = krate.fun_decls.get(*callee_id) {
                            let name = format_name(&decl.item_meta.name);
                            if name.contains("thread::spawn") {
                                if let Some(Operand::Move(p) | Operand::Copy(p)) =
                                    call.args.get(0)
                                {
                                    if let Some(closure_local) = p.local_id() {
                                        if let Some(endpoint) =
                                            local_to_endpoint.get(&(fun_id, closure_local))
                                        {
                                            if endpoint.kind == EndpointKind::Sender {
                                                if let Some(lt) = lifetimes.get_mut(endpoint) {
                                                    lt.moved_to_thread = true;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    (local_to_endpoint, channels, lifetimes)
}

/// Detect orphan receivers: recv() exists in a function but no corresponding send()
/// is found in the same function for the same channel.
pub fn detect_orphan_senders(
    crate_data: &CrateData,
    callsites: &ChannelCallSites,
    local_to_endpoint: &FxHashMap<(FunDeclId, LocalId), EndpointId>,
    channels: &FxHashMap<ChannelId, (EndpointId, EndpointId)>,
    lifetimes: &FxHashMap<EndpointId, EndpointLifetime>,
) -> Vec<Report> {
    let mut reports = Vec::new();
    let krate = &crate_data.translated;

    for (fun_id, sites) in callsites {
        let Some(fun_decl) = krate.fun_decls.get(*fun_id) else {
            continue;
        };
        let Body::Unstructured(body) = &fun_decl.body else {
            continue;
        };

        let mut sender_syncs: FxHashMap<ChannelId, Vec<&ChannelCallSite>> = FxHashMap::default();
        let mut receiver_syncs: FxHashMap<ChannelId, Vec<&ChannelCallSite>> = FxHashMap::default();

        for site in sites {
            if let Some(ch_local) = site.ch_local {
                if let Some(endpoint) = local_to_endpoint.get(&(*fun_id, ch_local)) {
                    match site.api {
                        ChannelApi::Send => {
                            sender_syncs.entry(endpoint.channel).or_default().push(site);
                        }
                        ChannelApi::Recv => {
                            receiver_syncs.entry(endpoint.channel).or_default().push(site);
                        }
                        _ => {}
                    }
                }
            }
        }

        for (cid, recv_sites) in &receiver_syncs {
            let send_sites = sender_syncs.get(cid).map(|v| v.as_slice()).unwrap_or(&[]);
            if send_sites.is_empty() {
                // If the sender was moved to a thread, assume it may send there.
                let sender_sent_elsewhere = channels
                    .get(cid)
                    .and_then(|(sender, _)| lifetimes.get(sender))
                    .map(|lt| lt.moved_to_thread)
                    .unwrap_or(false);
                if sender_sent_elsewhere {
                    continue;
                }
                for recv_site in recv_sites {
                    let recv_span = span_for_loc(body, recv_site.loc);
                    let fn_name = format_name(&fun_decl.item_meta.name);
                    let diagnosis = crate::lockbud::report::OrphanSenderDiagnosis {
                        fn_name,
                        channel_type: "std::sync::mpsc::channel".to_owned(),
                        receiver_recv_span: format!("{:?}", recv_span),
                        explanation: "Receiver calls recv() but no corresponding Sender::send() is found in the same function".to_owned(),
                    };
                    reports.push(Report::OrphanSender(ReportContent::new(
                        "OrphanSender".to_owned(),
                        "Possibly".to_owned(),
                        diagnosis,
                        "Receiver calls recv() but no corresponding Sender::send() is found in the same function".to_owned(),
                    )));
                }
            }
        }
    }

    reports
}

/// Detect missing sends: Sender moved to a spawned thread but no send() found anywhere.
pub fn detect_missing_sends(
    _crate_data: &CrateData,
    _callsites: &ChannelCallSites,
    _local_to_endpoint: &FxHashMap<(FunDeclId, LocalId), EndpointId>,
    _channels: &FxHashMap<ChannelId, (EndpointId, EndpointId)>,
    lifetimes: &FxHashMap<EndpointId, EndpointLifetime>,
) -> Vec<Report> {
    let mut reports = Vec::new();

    for (endpoint, lt) in lifetimes {
        if endpoint.kind != EndpointKind::Sender {
            continue;
        }
        if lt.moved_to_thread && lt.sync_locs.is_empty() {
            let diagnosis = crate::lockbud::report::MissingSendDiagnosis {
                fn_name: String::new(),
                channel_type: "std::sync::mpsc::channel".to_owned(),
                sender_span: "creation site".to_owned(),
                explanation: "Sender was moved to a spawned thread but no send() call was found in any analyzed function".to_owned(),
            };
            reports.push(Report::MissingSend(ReportContent::new(
                "MissingSend".to_owned(),
                "Possibly".to_owned(),
                diagnosis,
                "Sender was moved to a spawned thread but no send() call was found".to_owned(),
            )));
        }
    }

    reports
}
