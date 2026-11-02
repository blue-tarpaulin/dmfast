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
    &public::compiler::perf::vecgrow::VECGROW,
    &public::compiler::perf::fscache::FSCACHE,
    &private::compiler::perf::resources::RESOURCES,
    &public::compiler::perf::dmbbuf::DMBBUF,
    &private::compiler::perf::slots::SLOTS,
    &private::compiler::perf::intern::REBUILD,
    &public::compiler::perf::lists::LISTS,
    &public::compiler::perf::listinit::LISTINIT,
    &public::compiler::perf::protos::PROTOS,
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
    &public::server::perf::varlist::VARLIST,
    &public::server::perf::dmicmp::DMICMP,
    &private::server::perf::iconstates::ICONSTATES,
    &public::server::perf::iconcopy::ICONCOPY,
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
    &public::compiler::fixes::indent::INDENT,
    &public::compiler::fixes::contline::CONTLINE,
    &public::compiler::fixes::macrospace::MACROSPACE,
    &public::compiler::fixes::typepath::TYPEPATH,
    &public::compiler::fixes::objworld::OBJWORLD,
    &private::compiler::fixes::procargs::PROCARGS,
    &public::compiler::fixes::mapcolors::MAPCOLORS,
    &public::compiler::fixes::regexstart::REGEXSTART,
    &public::compiler::fixes::varlimit::VARLIMIT,
];

/// DreamDaemon bug fixes: opt-in, listed in BUGFIXES.md, each with a test world in `tests/fixes/<key>.dme`.
pub static SERVER_FIXES: &[&Feature] = &[
    &public::server::fixes::urlcrlf::URLCRLF,
    &public::server::fixes::jsonutf::JSONUTF,
    &public::server::fixes::num2sig::NUM2SIG,
    &public::server::fixes::nulldiv::NULLDIV,
    &private::server::fixes::minmax::MINMAX,
    &public::server::fixes::cmperr::CMPERR,
    &public::server::fixes::pickweight::PICKWEIGHT,
    &private::server::fixes::initvar::INITVAR,
    &public::server::fixes::dbbignum::DBBIGNUM,
    &public::server::fixes::dbclose::DBCLOSE,
    &private::server::fixes::tick::TICKINIT,
    &private::server::fixes::tick::TICK0,
    &public::server::fixes::callref::CALLREF,
    &public::server::fixes::hubsock::HUBSOCK,
    &public::server::fixes::rscid::RSCID,
    &public::server::fixes::profself::PROFSELF,
    &public::server::fixes::iconwrap::ICONWRAP,
    &public::server::fixes::profcpu::PROFCPU,
];

/// Fixes for both programs.
pub static COMMON_FIXES: &[&Feature] = &[&public::common::fixes::inodes::INODES];
