<script setup lang="ts">
import { onBeforeUnmount, onMounted } from 'vue'
defineProps<{ open: boolean; title: string }>()
const emit = defineEmits<{ close: [] }>()
function onKeydown(event: KeyboardEvent) { if (event.key === 'Escape') emit('close') }
onMounted(() => window.addEventListener('keydown', onKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))
</script>

<template><Teleport to="body"><div v-if="open" class="ui-modal" role="dialog" aria-modal="true" :aria-labelledby="`ui-modal-title-${title}`" @click.self="emit('close')"><div class="ui-modal__surface"><header class="panel-head"><h3 :id="`ui-modal-title-${title}`">{{ title }}</h3><button class="text-button" type="button" aria-label="Close dialog" @click="emit('close')">CLOSE ×</button></header><div class="ui-modal__body"><slot /></div></div></div></Teleport></template>
