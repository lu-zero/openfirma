//! Runner for `firma __dns-stub`.

use std::process::ExitCode;

use firma_run::dns_stub::{DnsStubInput, execute_dns_stub};

use crate::args::run::DnsStubArgs;

/// Run the hidden DNS stub helper.
///
/// # Errors
///
/// Returns an error if the stub fails to bind or terminates abnormally.
pub fn run(args: DnsStubArgs) -> anyhow::Result<ExitCode> {
    let input = DnsStubInput {
        listen: args.listen,
        inherited_udp_fd: args.inherited_udp_fd,
        inherited_tcp_fd: args.inherited_tcp_fd,
    };
    match execute_dns_stub(&input) {
        Ok(code) => Ok(exit_code(code)),
        Err(error) => Err(anyhow::anyhow!("{error}")),
    }
}

fn exit_code(code: i32) -> ExitCode {
    if code == 0 {
        ExitCode::SUCCESS
    } else {
        let exit = u8::try_from(code).unwrap_or(1);
        ExitCode::from(exit)
    }
}
