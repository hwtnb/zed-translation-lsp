<!--
Retry only if the remote service
reports a temporary failure.
-->
<template>
  <p :data-retry_count="3">Retry</p>
</template>
<script setup lang="ts">
// Retry only if the remote service
// reports a temporary failure.
const retry_count: number = 3;
</script>
<style lang="scss">
/*
Retry only if the remote service
reports a temporary failure.
*/
</style>
