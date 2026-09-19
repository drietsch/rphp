<?php
// `umask()` answers the previous mask and, with no argument, reads the
// current one — which the C API only allows by setting it and putting it back.
$first = umask();
var_dump($first === umask());
var_dump(umask(0o022));
var_dump(umask(0o777));
var_dump(umask("18"));
var_dump(umask(null));
var_dump(umask($first) === $first);
