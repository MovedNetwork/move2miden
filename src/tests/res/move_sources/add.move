module addition::add {
    fun add(x: u32, y: u32): u32 {
      x + y
    }

    public entry fun main() {
        assert!(add(2, 3) == 5, 1);
    }
}
