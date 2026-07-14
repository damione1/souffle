<script lang="ts">
  import { isClickableTranscriptWord, tokenizeTranscriptWords } from "../../../utils/transcript-words";
  import DictionaryAliasPopover from "./DictionaryAliasPopover.svelte";

  let {
    text,
    onAddAlias,
    class: className = "",
  }: {
    text: string;
    onAddAlias: (term: string, pronunciation: string | null) => void | Promise<void>;
    class?: string;
  } = $props();

  /** Index into `tokens`, not the word text: duplicate spellings share text but
   * must not all open the popover together. */
  let openTokenIndex = $state<number | null>(null);
  let tokens = $derived(tokenizeTranscriptWords(text));

  function swallowPointerEvent(event: MouseEvent) {
    event.stopPropagation();
  }

  function openAlias(tokenIndex: number, event: MouseEvent) {
    event.stopPropagation();
    openTokenIndex = tokenIndex;
  }
</script>

<span class={className}>
  {#each tokens as token, i (i)}
    {#if token.kind === "gap"}
      {token.text}
    {:else if isClickableTranscriptWord(token.text)}
      <span class="relative inline">
        <button
          type="button"
          class="cursor-pointer rounded-[3px] border-0 bg-transparent p-0 font-inherit text-inherit hover:bg-accent/10 hover:text-accent"
          onmousedown={swallowPointerEvent}
          onclick={(event) => openAlias(i, event)}
          ondblclick={swallowPointerEvent}
        >
          {token.text}
        </button>
        {#if openTokenIndex === i}
          <DictionaryAliasPopover
            heardAs={token.text}
            onClose={() => { openTokenIndex = null; }}
            onSave={onAddAlias}
          />
        {/if}
      </span>
    {:else}
      {token.text}
    {/if}
  {/each}
</span>
