//! Every feature, by group, in install order (see `crate::base::driver`).

use dmfast_patches as public;
use dmfast_private as private;

use crate::base::feature::Feature;

/// Compiler speed patches: the `.dmb` stays identical to stock.
pub static COMPILER_PERF: &[&Feature] = &[
    &public::compiler::perf::poll::POLL,
    &public::compiler::perf::banner::BANNER,
    &private::compiler::perf::stdio::STDIO,
    &private::compiler::perf::strings::STRINGS,
    &private::compiler::perf::stdio::FTELL,
    &private::compiler::perf::strtab::STRTAB,
    &public::compiler::perf::fastexit::FASTEXIT,
    &public::compiler::perf::ctype::CTYPE,
    &public::compiler::perf::dirs::DIRS,
    &private::compiler::perf::filter::FILTER,
    &private::compiler::perf::parent::PARENT,
    &private::compiler::perf::children::CHILD_INDEX,
    &private::compiler::perf::leafskip::LEAFSKIP,
    &private::compiler::perf::vecgrow::VECGROW,
    &public::compiler::perf::fscache::FSCACHE,
    &private::compiler::perf::resources::RESOURCES,
    &private::compiler::perf::dmbbuf::DMBBUF,
    &private::compiler::perf::slots::SLOTS,
    &private::compiler::perf::intern::REBUILD,
    &private::compiler::perf::lists::LISTS,
    &public::compiler::perf::listinit::LISTINIT,
    &private::compiler::perf::protos::PROTOS,
    &private::compiler::perf::intern::INTERNIDX,
];

/// DreamDaemon speed patches: game behaviour is identical to stock.
pub static SERVER_PERF: &[&Feature] = &[
    &private::server::perf::dmbread::DMBREAD,
    &private::server::perf::procs::PROCS,
    &private::server::perf::strtree::STRTREE,
    &private::server::perf::sortmap::SORTMAP,
    &private::server::perf::crc::CRC,
    &private::server::perf::strtable::STRTABLE,
    &private::server::perf::varlist::VARLIST,
    &private::server::perf::dmicmp::DMICMP,
    &private::server::perf::iconstates::ICONSTATES,
    &private::server::perf::iconcopy::ICONCOPY,
    &public::server::perf::argload::ARGLOAD,
    &private::server::perf::seenclear::SEENCLEAR,
    &private::server::perf::mapskip::MAPSKIP,
    &public::server::perf::varread::VARREAD,
    &public::server::perf::getvar::GETVAR,
    &public::server::perf::turfvar::TURFVAR,
    &private::server::perf::turfpf::TURFPF,
    &private::server::perf::locidx::LOCIDX,
    &public::server::perf::truth::TRUTH,
];

/// Compiler bug fixes: opt-in, listed in BUGFIXES.md, each with a test world in `tests/fixes/<key>.dme`.
pub static COMPILER_FIXES: &[&Feature] = &[
    &private::compiler::fixes::indent::INDENT,
    &public::compiler::fixes::contline::CONTLINE,
    &private::compiler::fixes::macrospace::MACROSPACE,
    &public::compiler::fixes::typepath::TYPEPATH,
    &private::compiler::fixes::objworld::OBJWORLD,
    &private::compiler::fixes::procargs::PROCARGS,
    &public::compiler::fixes::mapcolors::MAPCOLORS,
    &public::compiler::fixes::regexstart::REGEXSTART,
    &private::compiler::fixes::varlimit::VARLIMIT,
];

/// DreamDaemon bug fixes: opt-in, listed in BUGFIXES.md, each with a test world in `tests/fixes/<key>.dme`.
pub static SERVER_FIXES: &[&Feature] = &[
    &public::server::fixes::urlcrlf::URLCRLF,
    &public::server::fixes::jsonutf::JSONUTF,
    &private::server::fixes::num2sig::NUM2SIG,
    &private::server::fixes::nulldiv::NULLDIV,
    &private::server::fixes::minmax::MINMAX,
    &private::server::fixes::cmperr::CMPERR,
    &public::server::fixes::pickweight::PICKWEIGHT,
    &private::server::fixes::initvar::INITVAR,
    &public::server::fixes::dbbignum::DBBIGNUM,
    &public::server::fixes::dbclose::DBCLOSE,
    &private::server::fixes::tick::TICKINIT,
    &private::server::fixes::tick::TICK0,
    &private::server::fixes::callref::CALLREF,
    &private::server::fixes::hubsock::HUBSOCK,
    &private::server::fixes::rscid::RSCID,
    &private::server::fixes::profself::PROFSELF,
    &public::server::fixes::iconwrap::ICONWRAP,
    &public::server::fixes::profcpu::PROFCPU,
];

/// Fixes for both programs.
pub static COMMON_FIXES: &[&Feature] = &[&public::common::fixes::inodes::INODES];
