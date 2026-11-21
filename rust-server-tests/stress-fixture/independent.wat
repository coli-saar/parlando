;; A one-turn guest written directly against the component canonical ABI.
(module
  (memory (export "memory") 1)
  (global $next (mut i32) (i32.const 2048))
  (global $played (mut i32) (i32.const 0))

  ;; Each two-byte view is a positional record. The role-specific names come from describe.
  ;; The remaining segments hold the rejection, MessagePack completion and metadata, opaque checkpoint.
  (data (i32.const 1024) "\91\00")
  (data (i32.const 1031) "\91\00")
  (data (i32.const 1038) "\91\01")
  (data (i32.const 1045) "\91\01")
  (data (i32.const 1052) "already_stepped")
  (data (i32.const 1067) "\c3")
  (data (i32.const 1071) "\01")
  (data (i32.const 1072) "\91\01")

  ;; The loader calls describe on a disposable instance. A session must start fresh.
  (global $described (mut i32) (i32.const 0))
  (data (i32.const 512) "{\"config\":{\"type\":\"null\"},\"action\":{\"type\":\"struct\",\"value\":[{\"name\":\"try\",\"schema\":{\"type\":\"bool\"}}]},\"view_a\":{\"type\":\"struct\",\"value\":[{\"name\":\"A\",\"schema\":{\"type\":\"u64\"}}]},\"view_b\":{\"type\":\"struct\",\"value\":[{\"name\":\"B\",\"schema\":{\"type\":\"u64\"}}]},\"completion\":{\"type\":\"bool\"},\"metadata\":{\"type\":\"struct\",\"value\":[{\"name\":\"sum\",\"schema\":{\"type\":\"u64\"}}]}}")
  (func (export "parlando:game/mechanics@4.0.0#describe") (result i32)
    (if (global.get $described) (then unreachable))
    (global.set $described (i32.const 1))
    (i32.store (i32.const 96) (i32.const 512))
    (i32.store (i32.const 100) (i32.const 358))
    (i32.const 96))

  ;; Allocate aligned canonical input buffers in this small test instance.
  (func (export "cabi_realloc")
    (param $old i32) (param $old_len i32) (param $alignment i32) (param $size i32)
    (result i32)
    (local $address i32)
    (local $mask i32)
    (local.set $mask (i32.sub (local.get $alignment) (i32.const 1)))
    (local.set $address
      (i32.and (i32.add (global.get $next) (local.get $mask))
        (i32.xor (local.get $mask) (i32.const -1))))
    (global.set $next (i32.add (local.get $address) (local.get $size)))
    (if (i32.ne (local.get $old_len) (i32.const 0))
      (then (memory.copy (local.get $address) (local.get $old) (local.get $old_len))))
    (local.get $address))

  ;; Address 128 holds the result tag. The success payload starts at 132,
  ;; followed by the optional checkpoint, both views, completion, and metadata.
  (func $frame (result i32)
    (i32.store (i32.const 128) (i32.const 0))
    (i32.store8 (i32.const 132) (global.get $played))
    (i32.store (i32.const 136) (i32.const 1071))
    (i32.store (i32.const 140) (i32.const 1))
    (i32.store (i32.const 144)
      (select (i32.const 1038) (i32.const 1024) (global.get $played)))
    (i32.store (i32.const 148)
      (i32.const 2))
    (i32.store (i32.const 152)
      (select (i32.const 1045) (i32.const 1031) (global.get $played)))
    (i32.store (i32.const 156)
      (i32.const 2))
    (i32.store8 (i32.const 160) (global.get $played))
    (i32.store (i32.const 164) (i32.const 1067))
    (i32.store (i32.const 168) (i32.const 1))
    (i32.store8 (i32.const 172) (global.get $played))
    (i32.store (i32.const 176) (i32.const 1072))
    (i32.store (i32.const 180) (i32.const 2))
    (i32.const 128))

  ;; Validate canonical input bytes for the MessagePack nil configuration.
  (func (export "parlando:game/mechanics@4.0.0#init")
    (param $config i32) (param $len i32) (param i64) (result i32)
    (if (global.get $described) (then unreachable))
    (if (i32.ne (local.get $len) (i32.const 1)) (then unreachable))
    (if (i32.ne (i32.load8_u (local.get $config)) (i32.const 0xc0)) (then unreachable))
    (global.set $played (i32.const 0))
    (call $frame))

  ;; The only emitted checkpoint is the terminal flag.
  (func (export "parlando:game/mechanics@4.0.0#restore")
    (param $checkpoint i32) (param $len i32) (result i32)
    (if (i32.ne (local.get $len) (i32.const 1)) (then unreachable))
    (global.set $played (i32.load8_u (local.get $checkpoint)))
    (i32.const 192))

  ;; The compact action [true], publicly named {"try":true}, ends the session once.
  (func (export "parlando:game/mechanics@4.0.0#step")
    (param i32) (param $action i32) (param $len i32) (result i32)
    (if (i32.ne (local.get $len) (i32.const 2)) (then unreachable))
    (if (i32.ne (i32.load16_u (local.get $action)) (i32.const 0xc391)) (then unreachable))
    (if (result i32) (global.get $played)
      (then
        (i32.store (i32.const 128) (i32.const 1))
        (i32.store (i32.const 132) (i32.const 1052))
        (i32.store (i32.const 136) (i32.const 15))
        (i32.const 128))
      (else
        (global.set $played (i32.const 1))
        (call $frame))))
)
