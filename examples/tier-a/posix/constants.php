<?php
// Every POSIX_* constant php declares on this platform, with its value.
foreach ([
    'POSIX_F_OK', 'POSIX_X_OK', 'POSIX_W_OK', 'POSIX_R_OK',
    'POSIX_S_IFREG', 'POSIX_S_IFCHR', 'POSIX_S_IFBLK', 'POSIX_S_IFIFO', 'POSIX_S_IFSOCK',
    'POSIX_RLIMIT_AS', 'POSIX_RLIMIT_CORE', 'POSIX_RLIMIT_CPU', 'POSIX_RLIMIT_DATA',
    'POSIX_RLIMIT_FSIZE', 'POSIX_RLIMIT_MEMLOCK', 'POSIX_RLIMIT_NOFILE', 'POSIX_RLIMIT_NPROC',
    'POSIX_RLIMIT_RSS', 'POSIX_RLIMIT_STACK', 'POSIX_RLIMIT_INFINITY',
    'POSIX_SC_ARG_MAX', 'POSIX_SC_CHILD_MAX', 'POSIX_SC_CLK_TCK', 'POSIX_SC_PAGESIZE',
    'POSIX_SC_NPROCESSORS_CONF', 'POSIX_SC_NPROCESSORS_ONLN', 'POSIX_SC_OPEN_MAX',
    'POSIX_PC_LINK_MAX', 'POSIX_PC_MAX_CANON', 'POSIX_PC_MAX_INPUT', 'POSIX_PC_NAME_MAX',
    'POSIX_PC_PATH_MAX', 'POSIX_PC_PIPE_BUF', 'POSIX_PC_CHOWN_RESTRICTED', 'POSIX_PC_NO_TRUNC',
    'POSIX_PC_ALLOC_SIZE_MIN', 'POSIX_PC_SYMLINK_MAX',
] as $name) {
    echo $name, ' = ', var_export(constant($name), true), "\n";
}
foreach (['posix_getpid', 'posix_errno', 'posix_sysconf', 'posix_pathconf', 'posix_fpathconf', 'posix_initgroups', 'posix_eaccess'] as $f) {
    echo $f, ' ', var_export(function_exists($f), true), "\n";
}
