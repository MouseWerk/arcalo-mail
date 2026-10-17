package de.mousewerk.arcalo.mail

import android.content.Context
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import org.json.JSONObject
import java.security.MessageDigest

/**
 * What Android knows about the installed app, for the update notice of the APK
 * (src-tauri/src/mobile/apknotice.rs): package, version, the SHA-256 of the signing certificate
 * and the package that installed it (Google Play: com.android.vending, which updates the app
 * itself; none: installed by hand from the GitHub release).
 */
object AppFacts {
    @JvmStatic
    fun facts(context: Context): String {
        val pm = context.packageManager
        val name = context.packageName
        val info: PackageInfo = if (Build.VERSION.SDK_INT >= 28) {
            pm.getPackageInfo(name, PackageManager.GET_SIGNING_CERTIFICATES)
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(name, PackageManager.GET_SIGNATURES)
        }
        val code = if (Build.VERSION.SDK_INT >= 28) info.longVersionCode else {
            @Suppress("DEPRECATION")
            info.versionCode.toLong()
        }
        val cert: ByteArray? = if (Build.VERSION.SDK_INT >= 28) {
            info.signingInfo?.let { s ->
                if (s.hasMultipleSigners()) s.apkContentsSigners.firstOrNull() else s.signingCertificateHistory.lastOrNull()
            }?.toByteArray()
        } else {
            @Suppress("DEPRECATION")
            info.signatures?.firstOrNull()?.toByteArray()
        }
        val installer = try {
            if (Build.VERSION.SDK_INT >= 30) pm.getInstallSourceInfo(name).installingPackageName
            else {
                @Suppress("DEPRECATION")
                pm.getInstallerPackageName(name)
            }
        } catch (e: Exception) {
            null
        }
        val json = JSONObject()
        json.put("package", name)
        json.put("versionCode", code)
        json.put("versionName", info.versionName ?: "")
        if (cert != null) {
            val digest = MessageDigest.getInstance("SHA-256").digest(cert)
            json.put("signer", digest.joinToString("") { "%02X".format(it) })
        }
        if (installer != null) json.put("installer", installer)
        return json.toString()
    }
}
