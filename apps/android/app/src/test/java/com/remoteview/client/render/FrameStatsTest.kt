package com.remoteview.client.render

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameStatsTest {
    @Test
    fun steadyStream() {
        val s = FrameStats()
        val step = 8_333_333L
        var t = 1_000_000_000L
        repeat(240) { s.record(t); t += step }
        val snap = s.snapshot(t - step)
        assertEquals(120f, snap.avgFps, 0.5f)
        assertEquals(120f, snap.low1Fps, 1f)
        assertEquals(0f, snap.varianceMs2, 0.01f)
        assertEquals(120f, snap.fpsNow, 2f)
    }

    @Test
    fun spikeLowersOnePercentLow() {
        val s = FrameStats()
        var t = 0L
        repeat(199) { s.record(t); t += 10_000_000L }
        s.record(t); t += 50_000_000L
        s.record(t)
        val snap = s.snapshot(t)
        assertTrue(snap.low1Fps < 40f)
        assertTrue(snap.avgFps > 90f)
        assertTrue(snap.varianceMs2 > 0f)
    }

    @Test
    fun emptyIsZero() {
        assertEquals(0f, FrameStats().snapshot(1_000L).fpsNow, 0f)
    }
}
