<?php echo "a\n"; $x = [1]; echo $x[5]; echo $undefined; trigger_error("dep", E_USER_DEPRECATED); echo "b\n"; error_log("to the log"); echo 1/0;
