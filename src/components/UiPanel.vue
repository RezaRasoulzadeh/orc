<script setup lang="ts">
withDefaults(defineProps<{ title?: string; eyebrow?: string; actions?: boolean }>(), { actions: false })
</script>

<template>
  <section class="ui-panel panel">
    <header v-if="title || eyebrow || actions" class="ui-panel__header panel-head">
      <div><span v-if="eyebrow" class="eyebrow">{{ eyebrow }}</span><h3 v-if="title">{{ title }}</h3></div>
      <div v-if="actions" class="ui-panel__actions"><slot name="actions" /></div>
    </header>
    <div class="ui-panel__body"><slot /></div>
  </section>
</template>
