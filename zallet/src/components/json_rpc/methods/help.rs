// See `generate_rpc_help()` in `build.rs` for how this is generated.
include!(concat!(env!("OUT_DIR"), "/rpc_help.rs"));

pub(crate) fn call(command: Option<&str>) -> String {
    if let Some(command) = command {
        match COMMANDS.get(command) {
            None => format!("help: unknown command: {command}\n"),
            Some(help_text) => format!("{command}\n\n{help_text}"),
        }
    } else {
        let mut commands = COMMANDS.entries().collect::<Vec<_>>();
        commands.sort_by_cached_key(|(command, _)| command.to_string());

        let mut ret = String::new();
        for (command, _) in commands {
            ret.push_str(command);
            ret.push('\n');
        }
        ret
    }
}
