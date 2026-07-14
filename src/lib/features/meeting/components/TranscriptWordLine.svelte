<script lang="ts">
  import {
    isClickableTranscriptWord,
    tokenizeTranscriptWords,
    type AnchorRect,
  } from "../../../utils";
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
  let openAnchorRect = $state<AnchorRect | null>(null);
  let tokens = $derived(tokenizeTranscriptWords(text));

  function swallowPointerEvent(event: MouseEvent) {
    event.stopPropagation();
  }

  function openAlias(tokenIndex: number, event: MouseEvent) {
    event.stopPropagation();
    const target = event.currentTarget as HTMLElement;
    openAnchorRect = target.getBoundingClientRect();
    openTokenIndex = tokenIndex;
  }

  function closeAlias() {
    openTokenIndex = null;
    openAnchorRect = null;
  }
</script>

<span class={className}>
  {#each tokens as token, i (i)}
    {#if token.kind === "gap"}
      {token.text}
    {:else if isClickableTranscriptWord(token.text)}
      <button
        type="button"
        class="cursor-pointer rounded-[3px] border-0 bg-transparent p-0 font-inherit text-inherit hover:bg-accent/10 hover:text-accent"
        onmousedown={swallowPointerEvent}
        onclick={(event) => openAlias(i, event)}
        ondblclick={swallowPointerEvent}
      >
        {token.text}
      </button>
      {#if openTokenIndex === i && openAnchorRect}
        <DictionaryAliasPopover
          heardAs={token.text}
          anchorRect={openAnchorRect}
          onClose={closeAlias}
          onSave={onAddAlias}
        />
      {/if}
    {:else}
      {token.text}
    {/if}
  {/each}
</span>
