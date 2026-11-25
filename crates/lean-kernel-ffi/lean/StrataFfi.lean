namespace StrataFfi

/-- Spike stand-in for a kernel step over canonical bytes: the input reversed, then the sum of its
bytes modulo 256. -/
def step (input : ByteArray) : ByteArray := Id.run do
  let n := input.size
  let mut out := ByteArray.emptyWithCapacity (n + 1)
  let mut sum : UInt8 := 0
  for i in [0:n] do
    let b := input.get! (n - 1 - i)
    out := out.push b
    sum := sum + b
  return out.push sum

@[export strata_ffi_step]
def stepExport (input : ByteArray) : ByteArray := step input

/-- Returns its input: measures the cost of the boundary alone. -/
@[export strata_ffi_identity]
def identity (input : ByteArray) : ByteArray := input

end StrataFfi
