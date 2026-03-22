<script lang="ts">
  let { text, label = "Copy" }: { text: string; label?: string } = $props();
  let copied = $state(false);

  function copy() {
    navigator.clipboard.writeText(text);
    copied = true;
    setTimeout(() => {
      copied = false;
    }, 1500);
  }
</script>

<button onclick={copy} class="btn">{copied ? "Copied!" : label}</button>
