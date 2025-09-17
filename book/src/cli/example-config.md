# The `example-config` command

`zallet example-config` generates an example configuration TOML file that can be used to
run Zallet.

The command takes one flag that is currently required: `-o/--output PATH` which specifies
where the generated config file should be written. The value `-` will write the config to
stdout.

> For the Zallet alpha releases, the command also currently takes another required flag
> `--this-is-alpha-code-and-you-will-need-to-recreate-the-example-later`.

The generated config file contains every available config option, along with their
documentation:

```toml
$ zallet example-config -o -
{{#include ../../../zallet/tests/cmd/example_config.out/zallet.toml::25}}
...
```
