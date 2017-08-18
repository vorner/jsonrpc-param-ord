A small collection of hacks to make clangd work.

The jsonrpc-param-ord can be placed in front of clangd to make sure the values
in the JSON objects are ordered in a way clangd can handle them.

The compile-commands-post utility can be run against the compile database and
it adds header files into it. They are not compiled directly, but clangd still
wants to know the flags.
