# The `@` suggestions check the members they list (#370): half a second after
# the list settles, then half a second more, one op 8 with their ids and a
# `gmv:` nonce. Under FAKE_LEFT=302 the answer says Bob left, and a pill
# naming a user no longer offers him for "b".
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /ro"
sleep 1
cmd "key Return"
cmd "type 3"
cmd "key Tab"
cmd "type si"
cmd "key Tab"
cmd "type D2"
cmd "key Return"
cmd "type wh"
cmd "key Tab"
cmd "type b"
sleep 1
cmd "suggestions $OUT/before.png"
sleep 2
cmd "backspace"
cmd "type b"
sleep 1
cmd "suggestions $OUT/after.png"
cmd "quit"
sleep 1
